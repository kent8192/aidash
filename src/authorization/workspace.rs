//! Transactional workspace access for authenticated subjects. Workspace read
//! permission covers the workspace's contents; event delivery has a separate
//! permission. Run contents additionally require run and memory read access.
use super::{
    access::Access,
    catalog,
    identity::SubjectIdentity,
    policy::{Evaluation, Resource},
};
use crate::{
    Error, Result, api_schema::StateResponse, config::NodeIdentity, domain::*, store::Store,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
pub struct Workspaces {
    pub store: Store,
    pub identity: SubjectIdentity,
}

impl Access {
    pub(crate) async fn create_workspace(
        &mut self,
        store: &Store,
        title: &str,
        goal: &str,
    ) -> Result<Workspace> {
        let id = Uuid::new_v4();
        let resource = self.resource(
            "workspace",
            id,
            json!({"owner":self.identity.subject,"workspace_id":id}),
        );
        self.require(&resource, "workspace.create").await?;
        let workspace = store
            .create_workspace_in(&mut self.tx, id, title, goal)
            .await?;
        sqlx::query("INSERT INTO authorization_workspaces(workspace_id,tenant,owner_subject) VALUES($1,$2,$3)")
            .bind(id).bind(&self.identity.tenant).bind(&self.identity.subject).execute(&mut *self.tx).await?;
        Ok(workspace)
    }

    async fn workspace_decide(
        &mut self,
        id: Uuid,
        action: &str,
        owner: Option<&str>,
    ) -> Result<bool> {
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
        self.record(&[(input, decision.clone())]).await?;
        Ok(decision.allowed)
    }

    async fn allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
        let owner: Option<String> = sqlx::query_scalar("SELECT owner_subject FROM authorization_workspaces WHERE workspace_id=$1 AND tenant=$2 FOR SHARE")
            .bind(id).bind(&self.identity.tenant).fetch_optional(&mut *self.tx).await?;
        self.workspace_decide(id, action, owner.as_deref()).await
    }

    async fn require_workspace(&mut self, id: Uuid, action: &str) -> Result<()> {
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
            if self.workspace_decide(id, action, Some(&owner)).await? {
                result.push(id);
            }
        }
        Ok(result)
    }

    pub(crate) async fn run_visible(&mut self, run: &Run) -> Result<bool> {
        if let Some(allowed) = self.cached_runs.get(&(run.workspace_id, run.id)) {
            return if *allowed {
                self.human_reads(run.workspace_id, run.id).await
            } else {
                Ok(false)
            };
        }
        let workspace = self.workspace(run.workspace_id).await?;
        let resource = self.resource("run", run.id, workspace.attributes.clone());
        let mut attributes = workspace.attributes;
        attributes["version"] = json!(run.agent_version);
        let memory = self.resource("memory", &run.agent_id, attributes);
        let allowed = self.decide(&resource, "run.read").await?
            && self.decide(&memory, "memory.read").await?;
        self.cached_runs.insert((run.workspace_id, run.id), allowed);
        Ok(allowed && self.human_reads(run.workspace_id, run.id).await?)
    }

    async fn event_visible(&mut self, event: &Event) -> Result<bool> {
        if event.kind.starts_with("generation.") {
            let Some(id) = event.data["id"]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                return Ok(false);
            };
            let job: Option<crate::generation::Request> = sqlx::query_as(
                "SELECT * FROM generation_requests WHERE id=$1 AND tenant=$2 AND workspace_id=$3",
            )
            .bind(id)
            .bind(&self.identity.tenant)
            .bind(event.workspace_id)
            .fetch_optional(&mut *self.tx)
            .await?;
            return match job {
                Some(job) => job.visible(self).await,
                None => Ok(false),
            };
        }
        if event.kind.starts_with("conversation.") {
            let Some(id) = event.data["id"]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                return Ok(false);
            };
            let conversation: Option<Conversation> =
                sqlx::query_as("SELECT * FROM conversations WHERE id=$1 AND workspace_id=$2")
                    .bind(id)
                    .bind(event.workspace_id)
                    .fetch_optional(&mut *self.tx)
                    .await?;
            let Some(conversation) = conversation else {
                return Ok(false);
            };
            let resource = self.conversation_resource(&conversation).await?;
            return self.decide(&resource, "conversation.read").await;
        }
        if event.kind.starts_with("human.") {
            let Some(id) = event.data["id"]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                return Ok(false);
            };
            let request: Option<HumanRequest> =
                sqlx::query_as("SELECT * FROM human_requests WHERE id=$1 AND workspace_id=$2")
                    .bind(id)
                    .bind(event.workspace_id)
                    .fetch_optional(&mut *self.tx)
                    .await?;
            let Some(request) = request else {
                return Ok(false);
            };
            if !self.human_visible(&request).await? {
                return Ok(false);
            }
        }
        let candidate = event.data.get("run_id").or_else(|| {
            (event.kind == "run.created")
                .then(|| event.data.get("id"))
                .flatten()
        });
        if candidate.is_none()
            && !["run.", "model.", "human."]
                .iter()
                .any(|p| event.kind.starts_with(p))
        {
            return Ok(true);
        }
        let Some(id) = candidate
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<Uuid>().ok())
        else {
            return Ok(false);
        };
        if let Some(workspace) = event.workspace_id
            && let Some(allowed) = self.cached_runs.get(&(workspace, id))
        {
            return if *allowed {
                self.human_reads(workspace, id).await
            } else {
                Ok(false)
            };
        }
        let run: Option<Run> = sqlx::query_as("SELECT * FROM runs WHERE id=$1 AND workspace_id=$2")
            .bind(id)
            .bind(event.workspace_id)
            .fetch_optional(&mut *self.tx)
            .await?;
        match run {
            Some(run) => self.run_visible(&run).await,
            None => Ok(false),
        }
    }

    async fn filter_events(&mut self, events: Vec<Event>) -> Result<Vec<Event>> {
        let mut result = vec![];
        for event in events {
            if self.event_visible(&event).await? {
                result.push(event);
            }
        }
        Ok(result)
    }

    pub(crate) async fn workspace_snapshot(&mut self, id: Uuid) -> Result<WorkspaceSnapshot> {
        let workspace = self.workspace(id).await?;
        self.require(&workspace, "workspace.read").await?;
        let events = if self.decide(&workspace, "workspace.events").await? {
            sqlx::query_as("SELECT * FROM (SELECT * FROM events WHERE workspace_id=$1 ORDER BY sequence DESC LIMIT 100) e ORDER BY sequence")
                .bind(id).fetch_all(&mut *self.tx).await?
        } else {
            vec![]
        };
        let events = self.filter_events(events).await?;
        Ok(WorkspaceSnapshot {
            workspace: sqlx::query_as("SELECT * FROM workspaces WHERE id=$1").bind(id).fetch_one(&mut *self.tx).await?,
            tasks: sqlx::query_as("SELECT * FROM tasks WHERE workspace_id=$1 ORDER BY created_at,id").bind(id).fetch_all(&mut *self.tx).await?,
            artifacts: sqlx::query_as("SELECT * FROM artifacts WHERE workspace_id=$1 ORDER BY created_at,id").bind(id).fetch_all(&mut *self.tx).await?,
            messages: sqlx::query_as("SELECT * FROM (SELECT * FROM messages WHERE workspace_id=$1 ORDER BY created_at DESC LIMIT 100) m ORDER BY created_at").bind(id).fetch_all(&mut *self.tx).await?,
            events,
        })
    }
}

impl Workspaces {
    pub async fn create(&self, title: &str, goal: &str) -> Result<Workspace> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = access.create_workspace(&self.store, title, goal).await;
        access.finish(result).await
    }

    pub async fn update(&self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = async {
            // The update response contains the complete workspace, including
            // fields that were not supplied by the caller.
            access.require_workspace(id, "workspace.read").await?;
            access.require_workspace(id, "workspace.update").await?;
            self.store
                .update_state_in(&mut access.tx, id, revision, state)
                .await
        }
        .await;
        access.finish(result).await
    }

    pub async fn create_task(&self, id: Uuid, input: &NewTask, key: Option<&str>) -> Result<Task> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = async {
            access.require_workspace(id, "task.create").await?;
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
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = async {
            access.require_workspace(id, "message.create").await?;
            self.store
                .message_in(&mut access.tx, id, &self.identity.subject, content, None)
                .await
        }
        .await;
        access.finish(result).await
    }

    pub async fn snapshot(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = access.workspace_snapshot(id).await;
        access.finish(result).await
    }

    pub async fn state(&self, node: NodeIdentity) -> Result<StateResponse> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = async {
            let visible = access.visible("workspace.read").await?;
            let mut event_workspaces = vec![];
            for id in &visible { if access.allowed(*id, "workspace.events").await? { event_workspaces.push(*id); } }
            let mut state=StateResponse {
                access: crate::api_schema::AccessProfile::Subject{tenant:self.identity.tenant.clone(),subject:self.identity.subject.clone()},
                node, registry: catalog::list_in(&mut access,&crate::registry::Search::default()).await?, peers: vec![], installations: vec![],
                workspaces: sqlx::query_as("SELECT * FROM workspaces WHERE id=ANY($1) ORDER BY created_at DESC,id").bind(&visible).fetch_all(&mut *access.tx).await?,
                tasks: sqlx::query_as("SELECT * FROM tasks WHERE workspace_id=ANY($1) ORDER BY created_at,id").bind(&visible).fetch_all(&mut *access.tx).await?,
                artifacts: sqlx::query_as("SELECT * FROM artifacts WHERE workspace_id=ANY($1) ORDER BY created_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                runs: sqlx::query_as("SELECT * FROM runs WHERE workspace_id=ANY($1) ORDER BY updated_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                human_requests: sqlx::query_as("SELECT * FROM human_requests WHERE workspace_id=ANY($1) ORDER BY created_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                conversations: sqlx::query_as("SELECT * FROM conversations WHERE workspace_id=ANY($1) ORDER BY created_at DESC,id LIMIT 500").bind(&visible).fetch_all(&mut *access.tx).await?,
                events: sqlx::query_as("SELECT * FROM (SELECT * FROM events WHERE workspace_id=ANY($1) ORDER BY sequence DESC LIMIT 100) e ORDER BY sequence").bind(&event_workspaces).fetch_all(&mut *access.tx).await?,
            };
            let mut runs=vec![];
            for run in state.runs {if access.run_visible(&run).await? {runs.push(run);}}
            let run_ids:std::collections::BTreeSet<Uuid>=runs.iter().map(|r|r.id).collect();
            state.runs=runs;
            let mut requests=vec![];
            for request in state.human_requests {
                if run_ids.contains(&request.run_id) {
                    let resource=access.human_resource(&request).await?;
                    if access.decide(&resource,"human.read").await? {requests.push(request);}
                }
            }
            state.human_requests=requests;
            let mut conversations=vec![];
            for conversation in state.conversations {
                let resource=access.conversation_resource(&conversation).await?;
                if access.decide(&resource,"conversation.read").await? {conversations.push(conversation);}
            }
            state.conversations=conversations;
            state.events=access.filter_events(state.events).await?;
            Ok(state)
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
        let mut access = Access::begin(&self.store, &self.identity).await?;
        access.audit = audit;
        let result = async {
            let visible = if let Some(id) = workspace {
                access.require_workspace(id, "workspace.read").await?;
                access.require_workspace(id, "workspace.events").await?;
                vec![id]
            } else {
                let mut visible = vec![];
                for id in access.visible("workspace.read").await? {
                    if access.allowed(id, "workspace.events").await? { visible.push(id); }
                }
                visible
            };
            let limit=limit.clamp(1,1000) as usize;
            let mut cursor=after.max(0);
            let mut result=vec![];
            // Continue past rejected events so they cannot starve later
            // permitted events or trap Last-Event-ID replay on an empty page.
            loop {
                let batch:Vec<Event>=sqlx::query_as("SELECT * FROM events WHERE workspace_id=ANY($1) AND sequence>$2 ORDER BY sequence LIMIT 500")
                    .bind(&visible).bind(cursor).fetch_all(&mut *access.tx).await?;
                let exhausted=batch.len()<500;
                for event in batch {
                    cursor=event.sequence;
                    if access.event_visible(&event).await? {result.push(event);}
                    if result.len()==limit {return Ok(result);}
                }
                if exhausted {break;}
            }
            Ok(result)
        }.await;
        access.finish(result).await
    }

    /// Stream batches can remain buffered while authority changes. Recheck at
    /// every emission, with no policy or credential lock held across a yield.
    pub async fn can_emit(&self, event: &Event) -> Result<bool> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = async {
            let Some(id) = event.workspace_id else {
                return Ok(false);
            };
            Ok(access.allowed(id, "workspace.read").await?
                && access.allowed(id, "workspace.events").await?
                && access.event_visible(event).await?)
        }
        .await;
        access.finish(result).await
    }
}
