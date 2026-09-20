//! Transactional workspace access for authenticated subjects. Workspace read
//! permission covers the workspace's contents; event delivery has a separate
//! permission. Global registry/mesh/administration routes remain operator-only.
use super::{
    Authorization, Snapshot,
    identity::SubjectIdentity,
    policy::{Evaluation, Resource},
};
use crate::{
    Error, Result, api_schema::StateResponse, config::NodeIdentity, domain::*, store::Store,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct Workspaces {
    pub store: Store,
    pub identity: SubjectIdentity,
}

struct Access {
    tx: Transaction<'static, Postgres>,
    identity: SubjectIdentity,
    snapshot: Snapshot,
    environment: Value,
    audit: bool,
}

impl Access {
    async fn begin(scope: &Workspaces) -> Result<Self> {
        let mut tx = scope.store.pool.begin().await?;
        let snapshot = scope.identity.lock(&mut tx).await?;
        Ok(Self {
            tx,
            identity: scope.identity.clone(),
            snapshot,
            environment: json!({"node_id":scope.store.node_id,"transport":"api"}),
            audit: true,
        })
    }

    async fn decide(&mut self, id: Uuid, action: &str, owner: Option<&str>) -> Result<bool> {
        let input = Evaluation {
            subject: self.identity.subject.clone(),
            action: action.into(),
            resource: Resource {
                tenant: self.identity.tenant.clone(),
                kind: "workspace".into(),
                id: id.to_string(),
                attributes: owner
                    .map(|owner| json!({"owner":owner,"workspace_id":id}))
                    .unwrap_or_else(|| json!({})),
            },
            environment: self.environment.clone(),
        };
        let mut decision = self.snapshot.bundle.evaluate(&input);
        decision.revision = self.snapshot.revision;
        if owner.is_none() {
            decision.allowed = false;
            decision.reason = "resource_unavailable".into();
        }
        if self.audit {
            Authorization::record(&mut self.tx, &self.identity.tenant, &input, &decision).await?;
        }
        Ok(decision.allowed)
    }

    async fn allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
        let owner: Option<String> = sqlx::query_scalar("SELECT owner_subject FROM authorization_workspaces WHERE workspace_id=$1 AND tenant=$2 FOR SHARE")
            .bind(id).bind(&self.identity.tenant).fetch_optional(&mut *self.tx).await?;
        self.decide(id, action, owner.as_deref()).await
    }

    async fn require(&mut self, id: Uuid, action: &str) -> Result<()> {
        if self.allowed(id, action).await? {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    async fn visible(&mut self, action: &str) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT workspace_id,owner_subject FROM authorization_workspaces WHERE tenant=$1 ORDER BY workspace_id FOR SHARE")
            .bind(&self.identity.tenant).fetch_all(&mut *self.tx).await?;
        let mut result = vec![];
        for (id, owner) in rows {
            if self.decide(id, action, Some(&owner)).await? {
                result.push(id);
            }
        }
        Ok(result)
    }

    /// Denials occur before writes and are committed as audit records. Other
    /// failures roll back both the attempted mutation and its decision audit.
    async fn finish<T>(self, result: Result<T>) -> Result<T> {
        if result.is_ok() || matches!(result, Err(Error::Forbidden)) {
            self.tx.commit().await?;
        } else {
            self.tx.rollback().await?;
        }
        result
    }
}

impl Workspaces {
    pub async fn create(&self, title: &str, goal: &str) -> Result<Workspace> {
        let mut access = Access::begin(self).await?;
        let result = async {
            let id = Uuid::new_v4();
            if !access.decide(id, "workspace.create", Some(&self.identity.subject)).await? { return Err(Error::Forbidden); }
            let workspace = self.store.create_workspace_in(&mut access.tx, id, title, goal).await?;
            sqlx::query("INSERT INTO authorization_workspaces(workspace_id,tenant,owner_subject) VALUES($1,$2,$3)")
                .bind(id).bind(&self.identity.tenant).bind(&self.identity.subject).execute(&mut *access.tx).await?;
            Ok(workspace)
        }.await;
        access.finish(result).await
    }

    pub async fn update(&self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
        let mut access = Access::begin(self).await?;
        let result = async {
            // The update response contains the complete workspace, including
            // fields that were not supplied by the caller.
            access.require(id, "workspace.read").await?;
            access.require(id, "workspace.update").await?;
            self.store
                .update_state_in(&mut access.tx, id, revision, state)
                .await
        }
        .await;
        access.finish(result).await
    }

    pub async fn create_task(&self, id: Uuid, input: &NewTask, key: Option<&str>) -> Result<Task> {
        let mut access = Access::begin(self).await?;
        let result = async {
            access.require(id, "task.create").await?;
            // Keep one caller's idempotency key from colliding with another
            // workspace, subject, or the legacy operator namespace.
            let key = key.map(|key| {
                format!(
                    "subject:{}",
                    crate::registry::digest(&json!([
                        self.identity.tenant,
                        self.identity.subject,
                        id,
                        key
                    ]))
                )
            });
            self.store
                .create_task_in(
                    &mut access.tx,
                    id,
                    input,
                    &self.identity.subject,
                    key.as_deref(),
                )
                .await
        }
        .await;
        access.finish(result).await
    }

    pub async fn message(&self, id: Uuid, content: &str) -> Result<()> {
        let mut access = Access::begin(self).await?;
        let result = async {
            access.require(id, "message.create").await?;
            self.store
                .message_in(&mut access.tx, id, &self.identity.subject, content, None)
                .await
        }
        .await;
        access.finish(result).await
    }

    pub async fn snapshot(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
        let mut access = Access::begin(self).await?;
        let result = async {
            access.require(id, "workspace.read").await?;
            let events = if access.allowed(id, "workspace.events").await? {
                sqlx::query_as("SELECT * FROM (SELECT * FROM events WHERE workspace_id=$1 ORDER BY sequence DESC LIMIT 100) e ORDER BY sequence")
                    .bind(id).fetch_all(&mut *access.tx).await?
            } else { vec![] };
            Ok(WorkspaceSnapshot {
                workspace: sqlx::query_as("SELECT * FROM workspaces WHERE id=$1").bind(id).fetch_one(&mut *access.tx).await?,
                tasks: sqlx::query_as("SELECT * FROM tasks WHERE workspace_id=$1 ORDER BY created_at,id").bind(id).fetch_all(&mut *access.tx).await?,
                artifacts: sqlx::query_as("SELECT * FROM artifacts WHERE workspace_id=$1 ORDER BY created_at,id").bind(id).fetch_all(&mut *access.tx).await?,
                messages: sqlx::query_as("SELECT * FROM (SELECT * FROM messages WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 100) m ORDER BY created_at").bind(id).fetch_all(&mut *access.tx).await?,
                events,
            })
        }.await;
        access.finish(result).await
    }

    pub async fn state(&self, node: NodeIdentity) -> Result<StateResponse> {
        let mut access = Access::begin(self).await?;
        let result = async {
            let visible = access.visible("workspace.read").await?;
            let mut event_workspaces = vec![];
            for id in &visible { if access.allowed(*id, "workspace.events").await? { event_workspaces.push(*id); } }
            Ok(StateResponse {
                node, registry: vec![], peers: vec![], installations: vec![],
                workspaces: sqlx::query_as("SELECT * FROM workspaces WHERE id=ANY($1) ORDER BY created_at DESC,id").bind(&visible).fetch_all(&mut *access.tx).await?,
                tasks: sqlx::query_as("SELECT * FROM tasks WHERE workspace_id=ANY($1) ORDER BY created_at,id").bind(&visible).fetch_all(&mut *access.tx).await?,
                artifacts: sqlx::query_as("SELECT * FROM artifacts WHERE workspace_id=ANY($1) ORDER BY created_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                runs: sqlx::query_as("SELECT * FROM runs WHERE workspace_id=ANY($1) ORDER BY updated_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                human_requests: sqlx::query_as("SELECT * FROM human_requests WHERE workspace_id=ANY($1) ORDER BY created_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                conversations: sqlx::query_as("SELECT * FROM conversations WHERE workspace_id=ANY($1) ORDER BY created_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                events: sqlx::query_as("SELECT * FROM (SELECT * FROM events WHERE workspace_id=ANY($1) ORDER BY sequence DESC LIMIT 100) e ORDER BY sequence").bind(&event_workspaces).fetch_all(&mut *access.tx).await?,
            })
        }.await;
        access.finish(result).await
    }

    pub async fn events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Event>> {
        self.read_events(after, workspace, limit, true).await
    }

    /// Polling itself does not append decision audits. Every delivered frame is
    /// separately checked and audited by can_emit, including buffered frames.
    pub async fn poll_events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Event>> {
        self.read_events(after, workspace, limit, false).await
    }

    async fn read_events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
        audit: bool,
    ) -> Result<Vec<Event>> {
        let mut access = Access::begin(self).await?;
        access.audit = audit;
        let result = async {
            let visible = if let Some(id) = workspace {
                access.require(id, "workspace.read").await?;
                access.require(id, "workspace.events").await?;
                vec![id]
            } else {
                let mut visible = vec![];
                for id in access.visible("workspace.read").await? {
                    if access.allowed(id, "workspace.events").await? { visible.push(id); }
                }
                visible
            };
            Ok(sqlx::query_as("SELECT * FROM events WHERE workspace_id=ANY($1) AND sequence>$2 ORDER BY sequence LIMIT $3")
                .bind(&visible).bind(after.max(0)).bind(limit.clamp(1,1000)).fetch_all(&mut *access.tx).await?)
        }.await;
        access.finish(result).await
    }

    /// Stream batches can remain buffered while authority changes. Recheck at
    /// every emission, with no policy or credential lock held across a yield.
    pub async fn can_emit(&self, event: &Event) -> Result<bool> {
        let mut access = Access::begin(self).await?;
        let result = async {
            let Some(id) = event.workspace_id else {
                return Ok(false);
            };
            Ok(access.allowed(id, "workspace.read").await?
                && access.allowed(id, "workspace.events").await?)
        }
        .await;
        access.finish(result).await
    }
}
