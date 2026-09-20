use sea_orm::sea_query::{
    Alias, Condition, Expr, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
};
pub(crate) mod access;
pub mod api;
pub mod catalog;
pub mod execution;
pub mod identity;
pub mod interaction;
pub mod policy;
pub mod workspace;

use crate::{Error, Result};
use policy::{Decision, Evaluation, PolicyBundle, identifier};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct Authorization {
    pub pool: PgPool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Snapshot {
    pub revision: i64,
    pub bundle: PolicyBundle,
}

impl Authorization {
    pub async fn replace(
        &self,
        tenant: &str,
        expected_revision: i64,
        bundle: PolicyBundle,
        actor: &str,
    ) -> Result<Snapshot> {
        bundle.validate()?;
        identifier(actor)?;
        if bundle.tenant != tenant || expected_revision < 0 || expected_revision == i64::MAX {
            return Err(Error::Invalid(
                "tenant mismatch or invalid expected revision".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let document = serde_json::to_value(&bundle)?;
        let revision: Option<i64> = if expected_revision == 0 {
            sqlx::query_scalar(
                &Query::insert()
                    .into_table(Alias::new("authorization_bundles"))
                    .columns([
                        Alias::new("tenant"),
                        Alias::new("revision"),
                        Alias::new("document"),
                    ])
                    .values_panic([
                        Expr::cust("$1").into(),
                        Expr::cust("1").into(),
                        Expr::cust("$2").into(),
                    ])
                    .on_conflict(
                        OnConflict::columns([Alias::new("tenant")])
                            .do_nothing()
                            .to_owned(),
                    )
                    .returning(Query::returning().columns([Alias::new("revision")]))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(tenant)
            .bind(&document)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            sqlx::query_scalar(
                &Query::update()
                    .table(Alias::new("authorization_bundles"))
                    .value(Alias::new("revision"), Expr::cust("revision+1"))
                    .value(Alias::new("document"), Expr::cust("$3"))
                    .value(Alias::new("updated_at"), Expr::cust("now()"))
                    .cond_where(
                        Condition::all()
                            .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                            .add(Expr::col(Alias::new("revision")).eq(Expr::cust("$2"))),
                    )
                    .returning(Query::returning().columns([Alias::new("revision")]))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(tenant)
            .bind(expected_revision)
            .bind(&document)
            .fetch_optional(&mut *tx)
            .await?
        };
        let revision =
            revision.ok_or_else(|| Error::Conflict("authorization revision changed".into()))?;
        sqlx::query(
            &Query::insert()
                .into_table(Alias::new("authorization_revisions"))
                .columns([
                    Alias::new("tenant"),
                    Alias::new("revision"),
                    Alias::new("document"),
                    Alias::new("actor"),
                ])
                .values_panic([
                    Expr::cust("$1").into(),
                    Expr::cust("$2").into(),
                    Expr::cust("$3").into(),
                    Expr::cust("$4").into(),
                ])
                .to_string(PostgresQueryBuilder),
        )
        .bind(tenant)
        .bind(revision)
        .bind(&document)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Snapshot { revision, bundle })
    }

    async fn load(tx: &mut Transaction<'_, Postgres>, tenant: &str) -> Result<Snapshot> {
        Self::load_with_mode(tx, tenant, false).await
    }

    pub(crate) async fn load_with_mode(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        exclusive: bool,
    ) -> Result<Snapshot> {
        identifier(tenant)?;
        let query = if exclusive {
            Query::select()
                .column(Alias::new("revision"))
                .column(Alias::new("document"))
                .from(Alias::new("authorization_bundles"))
                .cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                .lock(LockType::Update)
                .to_string(PostgresQueryBuilder)
        } else {
            Query::select()
                .column(Alias::new("revision"))
                .column(Alias::new("document"))
                .from(Alias::new("authorization_bundles"))
                .cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                .lock(LockType::Share)
                .to_string(PostgresQueryBuilder)
        };
        let row: Option<(i64, Value)> = sqlx::query_as(&query)
            .bind(tenant)
            .fetch_optional(&mut **tx)
            .await?;
        let (revision, document) =
            row.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
        Ok(Snapshot {
            revision,
            bundle: serde_json::from_value(document)?,
        })
    }

    pub async fn snapshot(&self, tenant: &str) -> Result<Snapshot> {
        let mut tx = self.pool.begin().await?;
        let snapshot = Self::load(&mut tx, tenant).await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    /// Check and audit under a shared policy lock. Mutation callers must use this
    /// same transaction so revocation cannot race their authorization decision.
    pub async fn evaluate_in_transaction(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        input: &Evaluation,
    ) -> Result<Decision> {
        input.validate()?;
        let snapshot = Self::load(tx, tenant).await?;
        let mut decision = snapshot.bundle.evaluate(input);
        decision.revision = snapshot.revision;
        Self::record(tx, tenant, input, &decision).await?;
        Ok(decision)
    }

    pub(super) async fn record(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        input: &Evaluation,
        decision: &Decision,
    ) -> Result<()> {
        // Hold allocation order through commit, matching the decision cursor.
        sqlx::query(
            &Query::select()
                .expr(Expr::cust("pg_advisory_xact_lock(71003202)"))
                .to_string(PostgresQueryBuilder),
        )
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            &Query::insert()
                .into_table(Alias::new("authorization_decisions"))
                .columns([
                    Alias::new("tenant"),
                    Alias::new("revision"),
                    Alias::new("subject"),
                    Alias::new("action"),
                    Alias::new("resource_kind"),
                    Alias::new("resource_id"),
                    Alias::new("decision"),
                ])
                .values_panic([
                    Expr::cust("$1").into(),
                    Expr::cust("$2").into(),
                    Expr::cust("$3").into(),
                    Expr::cust("$4").into(),
                    Expr::cust("$5").into(),
                    Expr::cust("$6").into(),
                    Expr::cust("$7").into(),
                ])
                .to_string(PostgresQueryBuilder),
        )
        .bind(tenant)
        .bind(decision.revision)
        .bind(&input.subject)
        .bind(&input.action)
        .bind(&input.resource.kind)
        .bind(&input.resource.id)
        .bind(serde_json::to_value(decision)?)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn evaluate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
        let mut tx = self.pool.begin().await?;
        let decision = Self::evaluate_in_transaction(&mut tx, tenant, input).await?;
        tx.commit().await?;
        Ok(decision)
    }

    pub async fn simulate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
        input.validate()?;
        let snapshot = self.snapshot(tenant).await?;
        let mut decision = snapshot.bundle.evaluate(input);
        decision.revision = snapshot.revision;
        Ok(decision)
    }

    pub async fn revisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
        identifier(tenant)?;
        Ok(sqlx::query_scalar(
            &Query::select()
                .expr(Expr::cust("to_jsonb(r)"))
                .from_as(Alias::new("authorization_revisions"), Alias::new("r"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("revision")).gt(Expr::cust("$2"))),
                )
                .order_by(Alias::new("revision"), Order::Asc)
                .limit(limit.clamp(1, 200) as u64)
                .to_string(PostgresQueryBuilder),
        )
        .bind(tenant)
        .bind(after)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn decisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
        identifier(tenant)?;
        Ok(sqlx::query_scalar(
            &Query::select()
                .expr(Expr::cust("to_jsonb(d)"))
                .from_as(Alias::new("authorization_decisions"), Alias::new("d"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("sequence")).gt(Expr::cust("$2"))),
                )
                .order_by(Alias::new("sequence"), Order::Asc)
                .limit(limit.clamp(1, 200) as u64)
                .to_string(PostgresQueryBuilder),
        )
        .bind(tenant)
        .bind(after)
        .fetch_all(&self.pool)
        .await?)
    }
}
