//! Transactional workspace access for authenticated subjects. Workspace read
//! permission opens the workspace; each contained resource and event requires
//! its own read permission. Run contents retain their source read requirements.
use super::{
    access::Access,
    catalog,
    identity::SubjectIdentity,
    policy::{Evaluation, Resource},
};
use crate::{
    Error, Result, api_schema::StateResponse, config::NodeIdentity, domain::*, store::Store,
};
use sea_orm::sea_query::{
    Alias, Asterisk, Condition, Expr, LockType, Order, PostgresQueryBuilder, Query,
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
        sqlx::query(
            &Query::insert()
                .into_table(Alias::new("authorization_workspaces"))
                .columns([
                    Alias::new("workspace_id"),
                    Alias::new("tenant"),
                    Alias::new("owner_subject"),
                ])
                .values_panic([Expr::cust("$1"), Expr::cust("$2"), Expr::cust("$3")])
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .bind(&self.identity.tenant)
        .bind(&self.identity.subject)
        .execute(&mut *self.tx)
        .await?;
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
        let owner: Option<String> = sqlx::query_scalar(
            &Query::select()
                .column(Alias::new("owner_subject"))
                .from(Alias::new("authorization_workspaces"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2"))),
                )
                .lock(LockType::Share)
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .bind(&self.identity.tenant)
        .fetch_optional(&mut *self.tx)
        .await?;
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
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            &Query::select()
                .column(Alias::new("workspace_id"))
                .column(Alias::new("owner_subject"))
                .from(Alias::new("authorization_workspaces"))
                .cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
                .order_by(Alias::new("workspace_id"), Order::Asc)
                .lock(LockType::Share)
                .to_string(PostgresQueryBuilder),
        )
        .bind(&self.identity.tenant)
        .fetch_all(&mut *self.tx)
        .await?;
        let mut result = vec![];
        for (id, owner) in rows {
            if self.workspace_decide(id, action, Some(&owner)).await? {
                result.push(id);
            }
        }
        Ok(result)
    }

    pub(crate) async fn run_visible(&mut self, run: &Run) -> Result<bool> {
        Ok(self.run_base_visible(run).await? && self.run_reads_visible(run.id).await?)
    }

    pub(crate) async fn run_base_visible(&mut self, run: &Run) -> Result<bool> {
        if let Some(allowed) = self.cached_runs.get(&(run.workspace_id, run.id)) {
            return if *allowed {
                self.human_reads(run.workspace_id, run.id).await
            } else {
                Ok(false)
            };
        }
        let workspace = self.workspace(run.workspace_id).await?;
        let resource = self.resource("run", run.id, workspace.attributes.clone());
        let memory = self.memory_resource(run).await?;
        let task: Option<Task> = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("tasks"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
                )
                .to_string(PostgresQueryBuilder),
        )
        .bind(run.task_id)
        .bind(run.workspace_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        let task_visible = match task {
            Some(task) => self.task_visible(&task).await?,
            None => false,
        };
        let allowed = task_visible
            && self.decide(&resource, "run.read").await?
            && self.decide(&memory, "memory.read").await?;
        self.cached_runs.insert((run.workspace_id, run.id), allowed);
        Ok(allowed && self.human_reads(run.workspace_id, run.id).await?)
    }

    async fn event_visible(&mut self, event: &Event) -> Result<bool> {
        if let Some(visible) = self.resource_event_visible(event).await? {
            return Ok(visible);
        }
        if event.kind.starts_with("generation.") {
            let Some(id) = event.data["id"]
                .as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
            else {
                return Ok(false);
            };
            let job: Option<crate::generation::Request> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("generation_requests"))
                    .cond_where(
                        Condition::all()
                            .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                            .add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
                            .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$3"))),
                    )
                    .to_string(PostgresQueryBuilder),
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
            let conversation: Option<Conversation> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("conversations"))
                    .cond_where(
                        Condition::all()
                            .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                            .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
                    )
                    .to_string(PostgresQueryBuilder),
            )
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
            let request: Option<HumanRequest> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("human_requests"))
                    .cond_where(
                        Condition::all()
                            .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                            .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
                    )
                    .to_string(PostgresQueryBuilder),
            )
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
        if candidate.is_none() {
            // Newly added event families require an explicit scoped reader.
            return Ok(matches!(
                event.kind.as_str(),
                "workspace.created" | "workspace.updated"
            ));
        }
        let Some(id) = candidate
            .and_then(Value::as_str)
            .and_then(|id| id.parse::<Uuid>().ok())
        else {
            return Ok(false);
        };
        let run: Option<Run> = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("runs"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
                )
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .bind(event.workspace_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        match run {
            Some(run) => self.run_visible(&run).await,
            None => Ok(false),
        }
    }

    async fn task_page(
        &mut self,
        workspaces: &[Uuid],
        offset: u64,
    ) -> Result<crate::api_schema::TaskPage> {
        let mut cursor = offset;
        let mut tasks = vec![];
        loop {
            let rows: Vec<Task> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("tasks"))
                    .and_where(Expr::cust("workspace_id=ANY($1)"))
                    .order_by(Alias::new("created_at"), Order::Desc)
                    .order_by(Alias::new("id"), Order::Desc)
                    .limit(500)
                    .offset(cursor)
                    .to_string(PostgresQueryBuilder),
            )
            .bind(workspaces)
            .fetch_all(&mut *self.tx)
            .await?;
            let exhausted = rows.len() < 500;
            for task in rows {
                cursor = cursor.saturating_add(1);
                if self.task_visible(&task).await? {
                    tasks.push(task);
                }
                if tasks.len() == 500 {
                    return Ok(crate::api_schema::TaskPage {
                        tasks,
                        next_offset: Some(cursor),
                    });
                }
            }
            if exhausted {
                return Ok(crate::api_schema::TaskPage {
                    tasks,
                    next_offset: None,
                });
            }
        }
    }

    async fn latest_visible_events(&mut self, workspaces: &[Uuid]) -> Result<Vec<Event>> {
        let mut cursor = i64::MAX;
        let mut result = vec![];
        loop {
            let rows: Vec<Event> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("events"))
                    .and_where(Expr::cust("workspace_id=ANY($1)"))
                    .and_where(Expr::col(Alias::new("sequence")).lt(Expr::cust("$2")))
                    .order_by(Alias::new("sequence"), Order::Desc)
                    .limit(100)
                    .to_string(PostgresQueryBuilder),
            )
            .bind(workspaces)
            .bind(cursor)
            .fetch_all(&mut *self.tx)
            .await?;
            let exhausted = rows.len() < 100;
            for event in rows {
                cursor = event.sequence;
                if self.event_visible(&event).await? {
                    result.push(event);
                }
                if result.len() == 100 {
                    break;
                }
            }
            if exhausted || result.len() == 100 {
                break;
            }
        }
        result.reverse();
        Ok(result)
    }

    async fn latest_visible_messages(&mut self, workspace: Uuid) -> Result<Vec<Message>> {
        let mut offset = 0;
        let mut result = vec![];
        loop {
            let rows: Vec<Message> = sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("messages"))
                    .and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                    .order_by(Alias::new("created_at"), Order::Desc)
                    .order_by(Alias::new("id"), Order::Desc)
                    .limit(100)
                    .offset(offset)
                    .to_string(PostgresQueryBuilder),
            )
            .bind(workspace)
            .fetch_all(&mut *self.tx)
            .await?;
            let exhausted = rows.len() < 100;
            for message in rows {
                if self.message_visible(&message).await? {
                    result.push(message);
                }
                if result.len() == 100 {
                    break;
                }
            }
            if exhausted || result.len() == 100 {
                break;
            }
            offset += 100;
        }
        result.reverse();
        Ok(result)
    }

    pub(crate) async fn workspace_snapshot(&mut self, id: Uuid) -> Result<WorkspaceSnapshot> {
        let workspace = self.workspace(id).await?;
        self.require(&workspace, "workspace.read").await?;
        let events = if self.decide(&workspace, "workspace.events").await? {
            self.latest_visible_events(&[id]).await?
        } else {
            vec![]
        };
        let mut snapshot = WorkspaceSnapshot {
            workspace: sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("workspaces"))
                    .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                    .to_string(PostgresQueryBuilder),
            )
            .bind(id)
            .fetch_one(&mut *self.tx)
            .await?,
            tasks: sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("tasks"))
                    .cond_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                    .order_by(Alias::new("created_at"), Order::Asc)
                    .order_by(Alias::new("id"), Order::Asc)
                    .to_string(PostgresQueryBuilder),
            )
            .bind(id)
            .fetch_all(&mut *self.tx)
            .await?,
            artifacts: sqlx::query_as(
                &Query::select()
                    .column(Asterisk)
                    .from(Alias::new("artifacts"))
                    .cond_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
                    .order_by(Alias::new("created_at"), Order::Asc)
                    .order_by(Alias::new("id"), Order::Asc)
                    .to_string(PostgresQueryBuilder),
            )
            .bind(id)
            .fetch_all(&mut *self.tx)
            .await?,
            messages: self.latest_visible_messages(id).await?,
            events,
        };
        let mut tasks = vec![];
        for task in snapshot.tasks {
            if self.task_visible(&task).await? {
                tasks.push(task);
            }
        }
        snapshot.tasks = tasks;
        let mut artifacts = vec![];
        for artifact in snapshot.artifacts {
            if self.artifact_visible(&artifact).await? {
                artifacts.push(artifact);
            }
        }
        snapshot.artifacts = artifacts;
        let mut messages = vec![];
        for message in snapshot.messages {
            if self.message_visible(&message).await? {
                messages.push(message);
            }
        }
        snapshot.messages = messages;
        self.track_snapshot(&snapshot).await?;
        Ok(snapshot)
    }
}

impl Workspaces {
    pub async fn task_page(&self, offset: u64) -> Result<crate::api_schema::TaskPage> {
        let mut access = Access::begin(&self.store, &self.identity).await?;
        let result = async {
            let workspaces = access.visible("workspace.read").await?;
            access.task_page(&workspaces, offset).await
        }
        .await;
        access.finish(result).await
    }

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
            access.require_workspace(id, "workspace.read").await?;
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
            access.related_tasks(id, input).await?;
            let task = self
                .store
                .create_task_in(
                    &mut access.tx,
                    id,
                    input,
                    &self.identity.subject,
                    key.as_deref(),
                )
                .await?;
            let resource = access.task_resource(&task).await?;
            access.require(&resource, "task.read").await?;
            Ok(task)
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
                .map(|_| ())
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
            for id in &visible {
                if access.allowed(*id, "workspace.events").await? {
                    event_workspaces.push(*id);
                }
            }
            let mut state = StateResponse {
                access: crate::api_schema::AccessProfile::Subject {
                    tenant: self.identity.tenant.clone(),
                    subject: self.identity.subject.clone(),
                },
                node,
                registry: catalog::list_in(&mut access, &crate::registry::Search::default())
                    .await?,
                peers: vec![],
                installations: vec![],
                workspaces: sqlx::query_as(
                    &Query::select()
                        .column(Asterisk)
                        .from(Alias::new("workspaces"))
                        .cond_where(Expr::cust("id=ANY($1)"))
                        .order_by(Alias::new("created_at"), Order::Desc)
                        .order_by(Alias::new("id"), Order::Asc)
                        .to_string(PostgresQueryBuilder),
                )
                .bind(&visible)
                .fetch_all(&mut *access.tx)
                .await?,
                tasks: access.task_page(&visible, 0).await?.tasks,
                artifacts: vec![],
                runs: vec![],
                human_requests: vec![],
                conversations: vec![],
                events: access.latest_visible_events(&event_workspaces).await?,
            };
            let mut offset = 0_u64;
            loop {
                let batch: Vec<Artifact> = sqlx::query_as(
                    &Query::select()
                        .column(Asterisk)
                        .from(Alias::new("artifacts"))
                        .cond_where(Expr::cust("workspace_id=ANY($1)"))
                        .order_by(Alias::new("created_at"), Order::Desc)
                        .order_by(Alias::new("id"), Order::Asc)
                        .limit(500)
                        .offset(offset)
                        .to_string(PostgresQueryBuilder),
                )
                .bind(&visible)
                .fetch_all(&mut *access.tx)
                .await?;
                let exhausted = batch.len() < 500;
                for artifact in batch {
                    if access.artifact_visible(&artifact).await? {
                        state.artifacts.push(artifact);
                    }
                    if state.artifacts.len() == 500 {
                        break;
                    }
                }
                if exhausted || state.artifacts.len() == 500 {
                    break;
                }
                offset += 500;
            }
            let mut offset = 0_i64;
            loop {
                let batch: Vec<Run> = sqlx::query_as(
                    &Query::select()
                        .column(Asterisk)
                        .from(Alias::new("runs"))
                        .cond_where(Expr::cust("workspace_id=ANY($1)"))
                        .order_by(Alias::new("updated_at"), Order::Desc)
                        .order_by(Alias::new("id"), Order::Asc)
                        .limit(500)
                        .offset(offset as u64)
                        .to_string(PostgresQueryBuilder),
                )
                .bind(&visible)
                .fetch_all(&mut *access.tx)
                .await?;
                let exhausted = batch.len() < 500;
                for run in batch {
                    if access.run_visible(&run).await? {
                        state.runs.push(run);
                    }
                    if state.runs.len() == 500 {
                        break;
                    }
                }
                if exhausted || state.runs.len() == 500 {
                    break;
                }
                offset += 500;
            }
            let run_ids: Vec<Uuid> = state.runs.iter().map(|run| run.id).collect();
            let mut offset = 0_i64;
            loop {
                let batch: Vec<HumanRequest> = sqlx::query_as(
                    &Query::select()
                        .column(Asterisk)
                        .from(Alias::new("human_requests"))
                        .cond_where(Expr::cust("run_id=ANY($1)"))
                        .order_by(Alias::new("created_at"), Order::Desc)
                        .order_by(Alias::new("id"), Order::Asc)
                        .limit(500)
                        .offset(offset as u64)
                        .to_string(PostgresQueryBuilder),
                )
                .bind(&run_ids)
                .fetch_all(&mut *access.tx)
                .await?;
                let exhausted = batch.len() < 500;
                for request in batch {
                    let resource = access.human_resource(&request).await?;
                    if access.decide(&resource, "human.read").await? {
                        state.human_requests.push(request);
                    }
                    if state.human_requests.len() == 500 {
                        break;
                    }
                }
                if exhausted || state.human_requests.len() == 500 {
                    break;
                }
                offset += 500;
            }
            let mut offset = 0_i64;
            loop {
                let batch: Vec<Conversation> = sqlx::query_as(
                    &Query::select()
                        .column(Asterisk)
                        .from(Alias::new("conversations"))
                        .cond_where(Expr::cust("workspace_id=ANY($1)"))
                        .order_by(Alias::new("created_at"), Order::Desc)
                        .order_by(Alias::new("id"), Order::Asc)
                        .limit(500)
                        .offset(offset as u64)
                        .to_string(PostgresQueryBuilder),
                )
                .bind(&visible)
                .fetch_all(&mut *access.tx)
                .await?;
                let exhausted = batch.len() < 500;
                for conversation in batch {
                    let resource = access.conversation_resource(&conversation).await?;
                    if access.decide(&resource, "conversation.read").await? {
                        state.conversations.push(conversation);
                    }
                    if state.conversations.len() == 500 {
                        break;
                    }
                }
                if exhausted || state.conversations.len() == 500 {
                    break;
                }
                offset += 500;
            }

            Ok(state)
        }
        .await;
        access.finish(result).await
    }

    pub async fn events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Event>> {
        self.read_events(after, workspace, limit, true)
            .await
            .map(|(events, _)| events)
    }

    /// Polling itself does not append decision audits. Every delivered frame is
    /// separately checked and audited by can_emit, including buffered frames.
    pub async fn poll_events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<Event>, i64)> {
        self.read_events(after, workspace, limit, false).await
    }

    async fn read_events(
        &self,
        after: i64,
        workspace: Option<Uuid>,
        limit: i64,
        audit: bool,
    ) -> Result<(Vec<Event>, i64)> {
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
                    if access.allowed(id, "workspace.events").await? {
                        visible.push(id);
                    }
                }
                visible
            };
            let limit = limit.clamp(1, 1000) as usize;
            let mut cursor = after.max(0);
            let mut result = vec![];
            // Continue past rejected events so they cannot starve later
            // permitted events or trap Last-Event-ID replay on an empty page.
            loop {
                let batch: Vec<Event> = sqlx::query_as(
                    &Query::select()
                        .column(Asterisk)
                        .from(Alias::new("events"))
                        .cond_where(
                            Condition::all()
                                .add(Expr::cust("workspace_id=ANY($1)"))
                                .add(Expr::col(Alias::new("sequence")).gt(Expr::cust("$2"))),
                        )
                        .order_by(Alias::new("sequence"), Order::Asc)
                        .limit(500)
                        .to_string(PostgresQueryBuilder),
                )
                .bind(&visible)
                .bind(cursor)
                .fetch_all(&mut *access.tx)
                .await?;
                let exhausted = batch.len() < 500;
                for event in batch {
                    cursor = event.sequence;
                    if access.event_visible(&event).await? {
                        result.push(event);
                    }
                    if result.len() == limit {
                        return Ok((result, cursor));
                    }
                }
                if exhausted {
                    break;
                }
            }
            Ok((result, cursor))
        }
        .await;
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
