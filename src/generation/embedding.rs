//! Pinned embedding providers and durable, ancestor-intersected reservations.
use crate::{
    Error, Result,
    authorization::{access::Access, catalog},
    registry::EntityRef,
    semantic::EmbeddingConfig,
    store::Store,
};
use sqlx::PgPool;
use uuid::Uuid;

pub(crate) enum Origin {
    Query(Option<Uuid>),
    Index(Uuid),
}
pub(crate) struct Reservation {
    pool: PgPool,
    requests: Vec<Uuid>,
    attempt: Uuid,
    amount: i64,
}
impl Reservation {
    pub(crate) async fn settle(self, reported: Option<u64>) -> Result<()> {
        let valid = reported.filter(|amount| *amount > 0 && *amount <= self.amount as u64);
        let refund = valid.map_or(0, |amount| self.amount - amount as i64);
        let mut tx = self.pool.begin().await?;
        for request in self.requests {
            sqlx::query(
                "UPDATE generation_budgets SET used_tokens=used_tokens-$2 WHERE request_id=$1",
            )
            .bind(request)
            .bind(refund)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE generation_embedding_usage SET reported_tokens=$3 WHERE request_id=$1 AND attempt_id=$2")
                .bind(request).bind(self.attempt).bind(reported.and_then(|amount| i64::try_from(amount).ok())).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        if reported.is_some_and(|amount| amount > self.amount as u64) {
            return Err(Error::Invalid(
                "embedding usage exceeded reserved input limits".into(),
            ));
        }
        Ok(())
    }
}
pub(crate) async fn reserve(
    access: &mut Access,
    store: &Store,
    workspace: Uuid,
    config: &EmbeddingConfig,
    text: &str,
    origin: Origin,
) -> Result<Option<Reservation>> {
    let jobs: Vec<super::Request> = sqlx::query_as("SELECT * FROM generation_requests WHERE tenant=$1 AND ($2 || '/agents/' || agent_id || '@' || agent_version)=ANY($3) ORDER BY id")
        .bind(&access.identity.tenant).bind(&store.node_id).bind(&access.subjects).fetch_all(&mut *access.tx).await?;
    let Some(first) = jobs.first() else {
        return Ok(None);
    };
    super::provision::require_live(
        access,
        &store.node_id,
        first.task_id,
        &EntityRef {
            id: first.agent_id.clone(),
            version: first.agent_version.clone(),
        },
    )
    .await?;
    let mut reference = None;
    for job in &jobs {
        let spec: serde_json::Value = sqlx::query_scalar("SELECT spec FROM generation_policy_history WHERE tenant=$1 AND policy_id=$2 AND revision=$3")
            .bind(&job.tenant).bind(&job.policy_id).bind(job.policy_revision).fetch_one(&mut *access.tx).await?;
        let spec: super::policy::Spec = serde_json::from_value(spec)?;
        let approved = spec.embedding.ok_or_else(|| {
            Error::Invalid(
                "generated semantic memory requires an approved embedding provider".into(),
            )
        })?;
        if reference
            .as_ref()
            .is_some_and(|reference| reference != &approved.provider)
        {
            return Err(Error::Forbidden);
        }
        reference = Some(approved.provider);
    }
    let reference = reference.ok_or(Error::Forbidden)?;
    catalog::entry(access, &reference, "registry.read").await?;
    let entry = catalog::entry(access, &reference, "embedding.invoke").await?;
    if entry.kind != "embedding"
        || serde_json::from_value::<EmbeddingConfig>(entry.config)? != *config
    {
        return Err(Error::Forbidden);
    }
    let amount = text
        .len()
        .checked_add(1024)
        .and_then(|amount| i64::try_from(amount).ok())
        .ok_or_else(|| Error::Invalid("embedding input reservation overflow".into()))?;
    let attempt = Uuid::new_v4();
    let (purpose, run, source) = match origin {
        Origin::Query(run) => ("query", run, None),
        Origin::Index(entry) => ("index", None, Some(entry)),
    };
    let mut tx = store.pool.begin().await?;
    for job in &jobs {
        let reserved: Option<Uuid> = sqlx::query_scalar("UPDATE generation_budgets SET embedding_calls=embedding_calls+1,used_tokens=used_tokens+$2 WHERE request_id=$1 AND embedding_calls<embedding_call_limit AND token_limit-used_tokens >= $2 RETURNING request_id")
            .bind(job.id).bind(amount).fetch_optional(&mut *tx).await?;
        if reserved.is_none() {
            return Err(Error::Invalid(
                "generated embedding call or token budget exhausted".into(),
            ));
        }
        sqlx::query("INSERT INTO generation_embedding_usage(request_id,attempt_id,workspace_id,run_id,entry_id,purpose,provider_id,provider_version,request_bytes,reserved_tokens) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
            .bind(job.id).bind(attempt).bind(workspace).bind(run).bind(source).bind(purpose).bind(&reference.id).bind(&reference.version).bind(text.len() as i64).bind(amount).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Some(Reservation {
        pool: store.pool.clone(),
        requests: jobs.iter().map(|job| job.id).collect(),
        attempt,
        amount,
    }))
}
