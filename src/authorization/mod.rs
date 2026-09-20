pub mod api;
pub mod identity;
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

#[derive(Debug, Serialize, utoipa::ToSchema)]
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
            sqlx::query_scalar("INSERT INTO authorization_bundles(tenant,revision,document) VALUES($1,1,$2) ON CONFLICT(tenant) DO NOTHING RETURNING revision")
                .bind(tenant).bind(&document).fetch_optional(&mut *tx).await?
        } else {
            sqlx::query_scalar("UPDATE authorization_bundles SET revision=revision+1,document=$3,updated_at=now() WHERE tenant=$1 AND revision=$2 RETURNING revision")
                .bind(tenant).bind(expected_revision).bind(&document).fetch_optional(&mut *tx).await?
        };
        let revision =
            revision.ok_or_else(|| Error::Conflict("authorization revision changed".into()))?;
        sqlx::query("INSERT INTO authorization_revisions(tenant,revision,document,actor) VALUES($1,$2,$3,$4)")
            .bind(tenant).bind(revision).bind(&document).bind(actor).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Snapshot { revision, bundle })
    }

    async fn load(tx: &mut Transaction<'_, Postgres>, tenant: &str) -> Result<Snapshot> {
        identifier(tenant)?;
        let row: Option<(i64, Value)> = sqlx::query_as(
            "SELECT revision,document FROM authorization_bundles WHERE tenant=$1 FOR SHARE",
        )
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
        sqlx::query("INSERT INTO authorization_decisions(tenant,revision,subject,action,resource_kind,resource_id,decision) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(tenant).bind(decision.revision).bind(&input.subject).bind(&input.action)
            .bind(&input.resource.kind).bind(&input.resource.id).bind(serde_json::to_value(decision)?)
            .execute(&mut **tx).await?;
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
        Ok(sqlx::query_scalar("SELECT to_jsonb(r) FROM authorization_revisions r WHERE tenant=$1 AND revision>$2 ORDER BY revision LIMIT $3")
            .bind(tenant).bind(after).bind(limit.clamp(1,200)).fetch_all(&self.pool).await?)
    }

    pub async fn decisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
        identifier(tenant)?;
        Ok(sqlx::query_scalar("SELECT to_jsonb(d) FROM authorization_decisions d WHERE tenant=$1 AND sequence>$2 ORDER BY sequence LIMIT $3")
            .bind(tenant).bind(after).bind(limit.clamp(1,200)).fetch_all(&self.pool).await?)
    }
}
