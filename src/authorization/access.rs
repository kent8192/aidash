//! A live policy/credential lease used by typed execution boundaries.
use super::{
    Authorization, Snapshot,
    identity::SubjectIdentity,
    policy::{Decision, Evaluation, Resource},
};
use crate::{Error, Result, store::Store};
use sea_orm::sea_query::{Alias, Condition, Expr, LockType, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub(crate) struct Access {
    pub(super) remote_read_cache: std::collections::BTreeMap<(Uuid, String), bool>,
    pub(super) unavailable_peers: std::collections::BTreeSet<String>,
    pub(super) checking_reads: std::collections::BTreeSet<(Uuid, String)>,
    pub(super) peer_client: reqwest::Client,
    pub(super) node_id: String,
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
    pub cached_humans: std::collections::BTreeMap<Uuid, bool>,
    pending_decisions: Vec<(Evaluation, Decision)>,
    pub(super) pool: PgPool,
    pub read_run: Option<Uuid>,
    pub read_grant: Option<Uuid>,
    pub(super) environment: Value,
}

impl Access {
    // The same compound operation can narrow its subject chain or change the
    // workspace context. Never reuse a read decision from its earlier authority.
    pub(super) fn authority_context(&self) -> String {
        crate::registry::digest(
            &json!({"subjects":self.subjects,"context":self.context,"environment":self.environment}),
        )
    }
    pub async fn begin(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
        Self::begin_with_lock(store, identity, false).await
    }

    pub async fn begin_exclusive(store: &Store, identity: &SubjectIdentity) -> Result<Self> {
        Self::begin_with_lock(store, identity, true).await
    }

    async fn begin_with_lock(
        store: &Store,
        identity: &SubjectIdentity,
        exclusive: bool,
    ) -> Result<Self> {
        let mut tx = store.pool.begin().await?;
        let snapshot = identity.lock_with_mode(&mut tx, exclusive).await?;
        sqlx::query("SAVEPOINT authorization_operation")
            .execute(&mut *tx)
            .await?;
        Ok(Self {
            remote_read_cache: Default::default(),
            unavailable_peers: Default::default(),
            checking_reads: Default::default(),
            peer_client: store.semantic_client.clone(),
            node_id: store.node_id.clone(),
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
            cached_humans: Default::default(),
            pending_decisions: vec![],
            pool: store.pool.clone(),
            read_run: None,
            read_grant: None,
            environment: json!({"node_id":store.node_id,"transport":"api"}),
        })
    }

    // The caller retains the outer Access until this mutation commits. Reusing
    // its locks avoids queuing a second shared lock behind a waiting revoker.
    pub async fn under_lease(lease: &Self) -> Result<Self> {
        let mut tx = lease.pool.begin().await?;
        sqlx::query("SAVEPOINT authorization_operation")
            .execute(&mut *tx)
            .await?;
        Ok(Self {
            remote_read_cache: Default::default(),
            unavailable_peers: Default::default(),
            checking_reads: Default::default(),
            peer_client: lease.peer_client.clone(),
            node_id: lease.node_id.clone(),
            tx,
            identity: lease.identity.clone(),
            snapshot: lease.snapshot.clone(),
            subjects: lease.subjects.clone(),
            durable_audit: false,
            audit: true,
            context: lease.context.clone(),
            inherited_lease: true,
            approved_catalog: lease.approved_catalog.clone(),
            cached_runs: Default::default(),
            cached_humans: Default::default(),
            pending_decisions: vec![],
            pool: lease.pool.clone(),
            read_run: lease.read_run,
            read_grant: lease.read_grant,
            environment: lease.environment.clone(),
        })
    }

    pub fn resource(&self, kind: &str, id: impl ToString, mut attributes: Value) -> Resource {
        if let (Some(attributes), Some(context)) =
            (attributes.as_object_mut(), self.context.as_object())
        {
            for (key, value) in context {
                attributes
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
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
            Query::select()
                .column(Alias::new("owner_subject"))
                .from(Alias::new("authorization_workspaces"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2"))),
                )
                .to_string(PostgresQueryBuilder)
        } else {
            Query::select()
                .column(Alias::new("owner_subject"))
                .from(Alias::new("authorization_workspaces"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2"))),
                )
                .lock(LockType::Share)
                .to_string(PostgresQueryBuilder)
        };
        let owner: Option<String> = sqlx::query_scalar(&query)
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

    pub(crate) async fn record(&mut self, records: &[(Evaluation, Decision)]) -> Result<()> {
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
            self.pending_decisions.extend_from_slice(records);
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

    pub async fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        if result.is_ok() || matches!(result, Err(Error::Forbidden)) {
            // A compound operation can discover a denial after creating rows.
            // Retain the decision audit while removing every protected change.
            // Policy and credential locks predate the savepoint and remain held.
            if result.is_err() {
                sqlx::query("ROLLBACK TO SAVEPOINT authorization_operation")
                    .execute(&mut *self.tx)
                    .await?;
            }
            for (input, decision) in &self.pending_decisions {
                Authorization::record(&mut self.tx, &self.identity.tenant, input, decision).await?;
            }
            self.tx.commit().await?;
        } else {
            self.tx.rollback().await?;
        }
        result
    }
}
