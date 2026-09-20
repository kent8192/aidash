//! Reservations precede provider calls and survive process death. A successful
//! bounded usage report can refund unused tokens; unknown usage stays charged.
use crate::{
    Error, Result,
    authorization::access::Access,
    provider::{ModelRequest, ModelResponse},
    store::Store,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) struct Reservation {
    pool: PgPool,
    attempt: Uuid,
    requests: Vec<Uuid>,
    amount: i64,
    window: usize,
}
impl Reservation {
    pub fn check_request(&self, request: &ModelRequest) -> Result<()> {
        // UTF-8 bytes deliberately overestimate normal provider tokenization.
        // Leave framing space and refuse overlarge input before network I/O.
        if serde_json::to_vec(request)?.len().saturating_add(1024) > self.window {
            return Err(Error::Invalid(
                "generated model request exceeds its reserved context window".into(),
            ));
        }
        Ok(())
    }
    pub async fn settle(self, response: &ModelResponse) -> Result<()> {
        let reported = response
            .input_tokens
            .checked_add(response.output_tokens)
            .and_then(|n| i64::try_from(n).ok());
        // Missing, overflowing or out-of-contract usage never frees allowance.
        let refund = reported
            .filter(|n| response.usage_complete && *n > 0 && *n <= self.amount)
            .map_or(0, |n| self.amount - n);
        let mut tx = self.pool.begin().await?;
        for id in self.requests {
            sqlx::query(
                "UPDATE generation_budgets SET used_tokens=used_tokens-$2 WHERE request_id=$1",
            )
            .bind(id)
            .bind(refund)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE generation_usage SET reported_tokens=$3 WHERE request_id=$1 AND attempt_id=$2 AND reported_tokens IS NULL").bind(id).bind(self.attempt).bind(reported).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        if reported.is_some_and(|n| n > self.amount) {
            return Err(Error::External(
                "provider usage exceeded reserved model limits".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) async fn reserve(
    access: &mut Access,
    store: &Store,
    run: Uuid,
    attempt: Uuid,
    window: usize,
    output: u32,
) -> Result<Option<Reservation>> {
    let requests:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM generation_requests WHERE tenant=$1 AND ($2 || '/agents/' || agent_id || '@' || agent_version)=ANY($3) ORDER BY id")
        .bind(&access.identity.tenant).bind(&store.node_id).bind(&access.subjects).fetch_all(&mut *access.tx).await?;
    if requests.is_empty() {
        return Ok(None);
    }
    let amount = window
        .checked_add(output as usize)
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(|| Error::Invalid("model reservation overflow".into()))?;
    let mut tx = store.pool.begin().await?;
    for id in &requests {
        let reserved:Option<Uuid>=sqlx::query_scalar("UPDATE generation_budgets SET used_tokens=used_tokens+$2 WHERE request_id=$1 AND token_limit-used_tokens >= $2 RETURNING request_id")
            .bind(id).bind(amount).fetch_optional(&mut *tx).await?;
        if reserved.is_none() {
            return Err(Error::Invalid(
                "generated agent token budget exhausted".into(),
            ));
        }
        sqlx::query("INSERT INTO generation_usage(request_id,attempt_id,run_id,reserved_tokens) VALUES($1,$2,$3,$4)")
            .bind(id).bind(attempt).bind(run).bind(amount).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Some(Reservation {
        pool: store.pool.clone(),
        attempt,
        requests,
        amount,
        window,
    }))
}

/// A generated definition only approves its explicit model. Do not silently
/// send its history to a separately configured, unbudgeted compaction provider.
pub(crate) struct ApprovedCompactor<'a> {
    pub inner: &'a dyn crate::context::jev::JevAsker,
    pub generated: bool,
}
#[async_trait::async_trait]
impl crate::context::jev::JevAsker for ApprovedCompactor<'_> {
    async fn ask(
        &self,
        state: &serde_json::Value,
        questions: &crate::context::jev::Questions,
    ) -> Result<serde_json::Value> {
        if self.generated {
            return Err(Error::Invalid(
                "generated context requires a separately approved compaction provider".into(),
            ));
        }
        self.inner.ask(state, questions).await
    }
}
