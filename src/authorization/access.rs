//! A live policy/credential lease used by typed execution boundaries.
use super::{
    Authorization, Snapshot,
    identity::SubjectIdentity,
    policy::{Decision, Evaluation, Resource},
};
use crate::{Error, Result, store::Store};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub(crate) struct Access {
    pub tx: Transaction<'static, Postgres>,
    pub identity: SubjectIdentity,
    pub snapshot: Snapshot,
    pub subjects: Vec<String>,
    pub durable_audit: bool,
    pub audit: bool,
    pub context: Value,
    pub inherited_lease: bool,
    pub approved_catalog: std::collections::BTreeSet<(String, String)>,
    pub cached_runs: std::collections::BTreeMap<(Uuid, Uuid), bool>,
    pool: PgPool,
    pub(super) environment: Value,
}

impl Access {
    pub async fn begin(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
        let mut tx = store.pool.begin().await?;
        let snapshot = identity.lock(&mut tx).await?;
        Ok(Self {
            tx,
            identity: identity.clone(),
            snapshot,
            subjects: vec![identity.subject.clone()],
            durable_audit: false,
            audit: true,
            context: json!({}),
            inherited_lease: false,
            approved_catalog: Default::default(),
            cached_runs: Default::default(),
            pool: store.pool.clone(),
            environment: json!({"node_id":store.node_id,"transport":"api"}),
        })
    }

    // The caller retains the outer Access until this mutation commits. Reusing
    // its locks avoids queuing a second shared lock behind a waiting revoker.
    pub async fn under_lease(lease: &Self) -> Result<Self> {
        Ok(Self {
            tx: lease.pool.begin().await?,
            identity: lease.identity.clone(),
            snapshot: lease.snapshot.clone(),
            subjects: lease.subjects.clone(),
            durable_audit: false,
            audit: true,
            context: lease.context.clone(),
            inherited_lease: true,
            approved_catalog: lease.approved_catalog.clone(),
            cached_runs: Default::default(),
            pool: lease.pool.clone(),
            environment: lease.environment.clone(),
        })
    }

    pub fn resource(&self, kind: &str, id: impl ToString, mut attributes: Value) -> Resource {
        if let (Some(attributes), Some(context)) =
            (attributes.as_object_mut(), self.context.as_object())
        {
            attributes.extend(context.clone());
        }
        Resource {
            tenant: self.identity.tenant.clone(),
            kind: kind.into(),
            id: id.to_string(),
            attributes,
        }
    }

    pub fn worker(&mut self) {
        self.environment["transport"] = json!("worker");
    }

    pub async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
        if self.inherited_lease && self.context["workspace_id"] != id.to_string() {
            return Err(Error::Forbidden);
        }
        let query = if self.inherited_lease {
            "SELECT owner_subject FROM authorization_workspaces WHERE workspace_id=$1 AND tenant=$2"
        } else {
            "SELECT owner_subject FROM authorization_workspaces WHERE workspace_id=$1 AND tenant=$2 FOR SHARE"
        };
        let owner: Option<String> = sqlx::query_scalar(query)
            .bind(id)
            .bind(&self.identity.tenant)
            .fetch_optional(&mut *self.tx)
            .await?;
        let Some(owner) = owner else {
            return Err(Error::Forbidden);
        };
        Ok(self.resource("workspace", id, json!({"owner":owner,"workspace_id":id})))
    }

    pub async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
        let mut records = vec![];
        let mut allowed = true;
        for subject in &self.subjects {
            let input = Evaluation {
                subject: subject.clone(),
                action: action.into(),
                resource: resource.clone(),
                environment: self.environment.clone(),
            };
            let mut decision = self.snapshot.bundle.evaluate(&input);
            decision.revision = self.snapshot.revision;
            allowed &= decision.allowed;
            records.push((input, decision));
        }
        self.record(&records).await?;
        Ok(allowed)
    }

    async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
        if !self.audit {
            return Ok(());
        }
        // A worker step spans the existing invocation-start/effect/result
        // commits. Keep authority locked for that entire boundary, and commit
        // its decision audit before effects so a killed worker cannot lose it.
        if self.durable_audit {
            let mut audit = self.pool.begin().await?;
            for (input, decision) in records {
                Authorization::record(&mut audit, &self.identity.tenant, input, decision).await?;
            }
            audit.commit().await?;
        } else {
            for (input, decision) in records {
                Authorization::record(&mut self.tx, &self.identity.tenant, input, decision).await?;
            }
        }
        Ok(())
    }

    pub async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
        if self.decide(resource, action).await? {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    pub async fn finish<T>(self, result: Result<T>) -> Result<T> {
        if result.is_ok() || matches!(result, Err(Error::Forbidden)) {
            self.tx.commit().await?;
        } else {
            self.tx.rollback().await?;
        }
        result
    }
}
