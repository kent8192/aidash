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
                &sea_orm::sea_query::Query::update()
                    .table(sea_orm::sea_query::Alias::new("generation_budgets"))
                    .value(
                        sea_orm::sea_query::Alias::new("used_tokens"),
                        sea_orm::sea_query::Expr::cust("used_tokens - $2"),
                    )
                    .and_where(sea_orm::sea_query::Expr::cust("request_id = $1"))
                    .to_string(sea_orm::sea_query::PostgresQueryBuilder),
            )
            .bind(request)
            .bind(refund)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                &sea_orm::sea_query::Query::update()
                    .table(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
                    .value(
                        sea_orm::sea_query::Alias::new("reported_tokens"),
                        sea_orm::sea_query::Expr::cust("$3"),
                    )
                    .and_where(sea_orm::sea_query::Expr::cust(
                        "request_id = $1 AND attempt_id = $2",
                    ))
                    .to_string(sea_orm::sea_query::PostgresQueryBuilder),
            )
            .bind(request)
            .bind(self.attempt)
            .bind(reported.and_then(|amount| i64::try_from(amount).ok()))
            .execute(&mut *tx)
            .await?;
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
    let jobs: Vec<super::Request> = sqlx::query_as(
        &sea_orm::sea_query::Query::select()
            .expr(sea_orm::sea_query::SimpleExpr::from(
                sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
            ))
            .from(sea_orm::sea_query::Alias::new("generation_requests"))
            .and_where(sea_orm::sea_query::Expr::cust(
                "tenant = $1 AND ($2 || '/agents/' || agent_id || '@' || agent_version) = ANY($3)",
            ))
            .order_by_expr(
                sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
                    sea_orm::sea_query::Alias::new("id"),
                )),
                sea_orm::sea_query::Order::Asc,
            )
            .to_string(sea_orm::sea_query::PostgresQueryBuilder),
    )
    .bind(&access.identity.tenant)
    .bind(&store.node_id)
    .bind(&access.subjects)
    .fetch_all(&mut *access.tx)
    .await?;
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
        let spec: serde_json::Value = sqlx::query_scalar(
            &sea_orm::sea_query::Query::select()
                .expr(sea_orm::sea_query::SimpleExpr::from(
                    sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("spec")),
                ))
                .from(sea_orm::sea_query::Alias::new("generation_policy_history"))
                .and_where(sea_orm::sea_query::Expr::cust(
                    "tenant = $1 AND policy_id = $2 AND revision = $3",
                ))
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .bind(&job.tenant)
        .bind(&job.policy_id)
        .bind(job.policy_revision)
        .fetch_one(&mut *access.tx)
        .await?;
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
        let reserved: Option<Uuid> = sqlx::query_scalar(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("generation_budgets")).value(sea_orm::sea_query::Alias::new("embedding_calls"), sea_orm::sea_query::Expr::cust("embedding_calls + 1")).value(sea_orm::sea_query::Alias::new("used_tokens"), sea_orm::sea_query::Expr::cust("used_tokens + $2")).and_where(sea_orm::sea_query::Expr::cust("request_id = $1 AND embedding_calls < embedding_call_limit AND token_limit - used_tokens >= $2")).returning(sea_orm::sea_query::Query::returning().exprs([sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("request_id")))])).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(job.id).bind(amount).fetch_optional(&mut *tx).await?;
        if reserved.is_none() {
            return Err(Error::Invalid(
                "generated embedding call or token budget exhausted".into(),
            ));
        }
        sqlx::query(
            &sea_orm::sea_query::Query::insert()
                .into_table(sea_orm::sea_query::Alias::new("generation_embedding_usage"))
                .columns([
                    sea_orm::sea_query::Alias::new("request_id"),
                    sea_orm::sea_query::Alias::new("attempt_id"),
                    sea_orm::sea_query::Alias::new("workspace_id"),
                    sea_orm::sea_query::Alias::new("run_id"),
                    sea_orm::sea_query::Alias::new("entry_id"),
                    sea_orm::sea_query::Alias::new("purpose"),
                    sea_orm::sea_query::Alias::new("provider_id"),
                    sea_orm::sea_query::Alias::new("provider_version"),
                    sea_orm::sea_query::Alias::new("request_bytes"),
                    sea_orm::sea_query::Alias::new("reserved_tokens"),
                ])
                .values_panic([
                    sea_orm::sea_query::Expr::cust("$1"),
                    sea_orm::sea_query::Expr::cust("$2"),
                    sea_orm::sea_query::Expr::cust("$3"),
                    sea_orm::sea_query::Expr::cust("$4"),
                    sea_orm::sea_query::Expr::cust("$5"),
                    sea_orm::sea_query::Expr::cust("$6"),
                    sea_orm::sea_query::Expr::cust("$7"),
                    sea_orm::sea_query::Expr::cust("$8"),
                    sea_orm::sea_query::Expr::cust("$9"),
                    sea_orm::sea_query::Expr::cust("$10"),
                ])
                .to_string(sea_orm::sea_query::PostgresQueryBuilder),
        )
        .bind(job.id)
        .bind(attempt)
        .bind(workspace)
        .bind(run)
        .bind(source)
        .bind(purpose)
        .bind(&reference.id)
        .bind(&reference.version)
        .bind(text.len() as i64)
        .bind(amount)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Some(Reservation {
        pool: store.pool.clone(),
        requests: jobs.iter().map(|job| job.id).collect(),
        attempt,
        amount,
    }))
}
