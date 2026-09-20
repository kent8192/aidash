//! Resource attributes come from stored rows and their authoritative workspace.
use super::{access::Access, policy::Resource};
use crate::{
    Error, Result,
    domain::{Artifact, Event, Message, Task},
};
use serde_json::json;
use uuid::Uuid;

impl Access {
    pub(crate) async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
        let workspace = self.workspace(task.workspace_id).await?;
        let mut attributes = workspace.attributes;
        attributes["created_by"] = json!(task.created_by);
        attributes["task_id"] = json!(task.id);
        Ok(self.resource("task", task.id, attributes))
    }
    pub(crate) async fn task_read(&mut self, id: Uuid) -> Result<Task> {
        let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *self.tx)
            .await?
            .ok_or(Error::Forbidden)?;
        let resource = self.task_resource(&task).await?;
        self.require(&resource, "task.read").await?;
        Ok(task)
    }
    pub(crate) async fn task_visible(&mut self, task: &Task) -> Result<bool> {
        let resource = self.task_resource(task).await?;
        self.decide(&resource, "task.read").await
    }
    pub(crate) async fn related_tasks(
        &mut self,
        workspace: Uuid,
        input: &crate::domain::NewTask,
    ) -> Result<()> {
        for id in input.dependencies.iter().chain(input.parent_id.iter()) {
            if self.task_read(*id).await?.workspace_id != workspace {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    pub(crate) async fn artifact_resource(&mut self, artifact: &Artifact) -> Result<Resource> {
        let workspace = self.workspace(artifact.workspace_id).await?;
        let mut attributes = workspace.attributes;
        attributes["created_by"] = json!(artifact.created_by);
        attributes["task_id"] = json!(artifact.task_id);
        attributes["kind"] = json!(artifact.kind);
        Ok(self.resource("artifact", artifact.id, attributes))
    }
    pub(crate) async fn artifact_visible(&mut self, artifact: &Artifact) -> Result<bool> {
        let resource = self.artifact_resource(artifact).await?;
        if !self.decide(&resource, "artifact.read").await? {
            return Ok(false);
        }
        let task: Option<Task> =
            sqlx::query_as("SELECT * FROM tasks WHERE id=$1 AND workspace_id=$2")
                .bind(artifact.task_id)
                .bind(artifact.workspace_id)
                .fetch_optional(&mut *self.tx)
                .await?;
        match task {
            Some(task) => self.task_visible(&task).await,
            None => Ok(false),
        }
    }
    pub(crate) async fn memory_resource(&mut self, run: &crate::domain::Run) -> Result<Resource> {
        let workspace = self.workspace(run.workspace_id).await?;
        let mut attributes = workspace.attributes;
        attributes["created_by"] = json!(crate::domain::qualified_agent(
            &run.home_node,
            &run.agent_id,
            &run.agent_version
        ));
        attributes["version"] = json!(run.agent_version);
        Ok(self.resource("memory", &run.agent_id, attributes))
    }
    pub(crate) async fn artifact_creation_resource(
        &mut self,
        task: Uuid,
        creator: &str,
    ) -> Result<Resource> {
        let task = self.task_read(task).await?;
        let mut resource = self.task_resource(&task).await?;
        resource.kind = "artifact".into();
        resource.attributes["created_by"] = json!(creator);
        Ok(resource)
    }
    pub(crate) async fn message_resource(&mut self, message: &Message) -> Result<Resource> {
        let workspace = self.workspace(message.workspace_id).await?;
        let mut attributes = workspace.attributes;
        attributes["created_by"] = json!(message.sender);
        attributes["sender"] = json!(message.sender);
        Ok(self.resource("message", message.id, attributes))
    }
    pub(crate) async fn message_visible(&mut self, message: &Message) -> Result<bool> {
        let resource = self.message_resource(message).await?;
        self.decide(&resource, "message.read").await
    }
    pub(crate) async fn resource_event_visible(&mut self, event: &Event) -> Result<Option<bool>> {
        let id = |value: &serde_json::Value| value.as_str().and_then(|s| s.parse::<Uuid>().ok());
        if event.kind.starts_with("task.") {
            let task_id = id(&event.data["task"]["id"])
                .or_else(|| id(&event.data["task_id"]))
                .or_else(|| id(&event.data["id"]));
            let Some(task_id) = task_id else {
                return Ok(Some(false));
            };
            let task: Option<Task> =
                sqlx::query_as("SELECT * FROM tasks WHERE id=$1 AND workspace_id=$2")
                    .bind(task_id)
                    .bind(event.workspace_id)
                    .fetch_optional(&mut *self.tx)
                    .await?;
            let Some(task) = task else {
                return Ok(Some(false));
            };
            if !self.task_visible(&task).await? {
                return Ok(Some(false));
            }
            if let Some(artifact_id) = id(&event.data["artifact"]["id"]) {
                return Ok(Some(
                    self.artifact_id_visible(artifact_id, event.workspace_id)
                        .await?,
                ));
            }
            return Ok(Some(true));
        }
        if event.kind.starts_with("artifact.") {
            let Some(artifact_id) = id(&event.data["id"]) else {
                return Ok(Some(false));
            };
            return Ok(Some(
                self.artifact_id_visible(artifact_id, event.workspace_id)
                    .await?,
            ));
        }
        if event.kind == "message.created" {
            let messages: Vec<Message> = if let Some(message_id) = id(&event.data["id"]) {
                sqlx::query_as("SELECT * FROM messages WHERE id=$1 AND workspace_id=$2")
                    .bind(message_id)
                    .bind(event.workspace_id)
                    .fetch_all(&mut *self.tx)
                    .await?
            } else {
                // Older events contain no ID. Require every matching immutable
                // row; ambiguity must never allow a denied message to escape.
                sqlx::query_as(
                    "SELECT * FROM messages WHERE workspace_id=$1 AND sender=$2 AND content=$3",
                )
                .bind(event.workspace_id)
                .bind(event.data["sender"].as_str())
                .bind(event.data["content"].as_str())
                .fetch_all(&mut *self.tx)
                .await?
            };
            if messages.is_empty() {
                return Ok(Some(false));
            }
            for message in messages {
                if !self.message_visible(&message).await? {
                    return Ok(Some(false));
                }
            }
            return Ok(Some(true));
        }
        Ok(None)
    }
    async fn artifact_id_visible(&mut self, id: Uuid, workspace: Option<Uuid>) -> Result<bool> {
        let artifact: Option<Artifact> =
            sqlx::query_as("SELECT * FROM artifacts WHERE id=$1 AND workspace_id=$2")
                .bind(id)
                .bind(workspace)
                .fetch_optional(&mut *self.tx)
                .await?;
        match artifact {
            Some(artifact) => self.artifact_visible(&artifact).await,
            None => Ok(false),
        }
    }
}

impl Access {
    /// Commit membership before a provider/tool can copy these records into its
    /// journal. The worker still retains the enclosing live authority lease.
    pub(crate) async fn track_snapshot(
        &mut self,
        snapshot: &crate::domain::WorkspaceSnapshot,
    ) -> Result<()> {
        let Some(run) = self.read_run else {
            return Ok(());
        };
        let mut sources: std::collections::BTreeSet<(String, Uuid)> = snapshot
            .tasks
            .iter()
            .map(|r| ("task".into(), r.id))
            .chain(snapshot.artifacts.iter().map(|r| ("artifact".into(), r.id)))
            .chain(snapshot.messages.iter().map(|r| ("message".into(), r.id)))
            .collect();
        let id = |value: &serde_json::Value| value.as_str().and_then(|s| s.parse::<Uuid>().ok());
        for event in &snapshot.events {
            let source = if event.kind.starts_with("conversation.") {
                id(&event.data["id"]).map(|id| ("conversation", id))
            } else if event.kind.starts_with("generation.") {
                id(&event.data["id"]).map(|id| ("generation", id))
            } else if event.kind == "run.created" {
                id(&event.data["id"]).map(|id| ("run", id))
            } else {
                id(&event.data["run_id"]).map(|id| ("run", id))
            };
            if let Some((kind, id)) = source
                && (kind != "run" || id != run)
            {
                sources.insert((kind.into(), id));
            }
            if event.kind == "message.created" {
                if let Some(id) = id(&event.data["id"]) {
                    sources.insert(("message".into(), id));
                } else {
                    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM messages WHERE workspace_id=$1 AND sender=$2 AND content=$3")
                        .bind(event.workspace_id).bind(event.data["sender"].as_str()).bind(event.data["content"].as_str()).fetch_all(&mut *self.tx).await?;
                    sources.extend(ids.into_iter().map(|id| ("message".into(), id)));
                }
            }
        }
        let (kinds, ids): (Vec<_>, Vec<_>) = sources.into_iter().unzip();
        sqlx::query("INSERT INTO authorization_run_reads(run_id,workspace_id,resource_kind,resource_id) SELECT $1,$2,kind,id FROM UNNEST($3::text[],$4::uuid[]) AS s(kind,id) ON CONFLICT DO NOTHING")
            .bind(run).bind(snapshot.workspace.id).bind(kinds).bind(ids).execute(&self.pool).await?;
        Ok(())
    }
    /// Walk recorded run dependencies iteratively; cycles between observation
    /// journals must terminate without skipping any resource's current policy.
    pub(crate) async fn run_reads_visible(&mut self, run: Uuid) -> Result<bool> {
        let mut pending = vec![run];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(run) = pending.pop() {
            if !visited.insert(run) {
                continue;
            }
            let sources:Vec<(Uuid,String,Uuid)>=sqlx::query_as("SELECT workspace_id,resource_kind,resource_id FROM authorization_run_reads WHERE run_id=$1 ORDER BY resource_kind,resource_id").bind(run).fetch_all(&mut *self.tx).await?;
            for (workspace, kind, id) in sources {
                let allowed = match kind.as_str() {
                    "task" => {
                        let source: Option<Task> =
                            sqlx::query_as("SELECT * FROM tasks WHERE id=$1 AND workspace_id=$2")
                                .bind(id)
                                .bind(workspace)
                                .fetch_optional(&mut *self.tx)
                                .await?;
                        match source {
                            Some(source) => self.task_visible(&source).await?,
                            None => false,
                        }
                    }
                    "artifact" => self.artifact_id_visible(id, Some(workspace)).await?,
                    "message" => {
                        let source: Option<Message> = sqlx::query_as(
                            "SELECT * FROM messages WHERE id=$1 AND workspace_id=$2",
                        )
                        .bind(id)
                        .bind(workspace)
                        .fetch_optional(&mut *self.tx)
                        .await?;
                        match source {
                            Some(source) => self.message_visible(&source).await?,
                            None => false,
                        }
                    }
                    "run" => {
                        let source: Option<crate::domain::Run> =
                            sqlx::query_as("SELECT * FROM runs WHERE id=$1 AND workspace_id=$2")
                                .bind(id)
                                .bind(workspace)
                                .fetch_optional(&mut *self.tx)
                                .await?;
                        match source {
                            Some(source) => {
                                pending.push(source.id);
                                self.run_base_visible(&source).await?
                            }
                            None => false,
                        }
                    }
                    "conversation" => {
                        let source: Option<crate::domain::Conversation> = sqlx::query_as(
                            "SELECT * FROM conversations WHERE id=$1 AND workspace_id=$2",
                        )
                        .bind(id)
                        .bind(workspace)
                        .fetch_optional(&mut *self.tx)
                        .await?;
                        match source {
                            Some(source) => {
                                let resource = self.conversation_resource(&source).await?;
                                self.decide(&resource, "conversation.read").await?
                            }
                            None => false,
                        }
                    }
                    "generation" => {
                        let source: Option<crate::generation::Request> = sqlx::query_as(
                            "SELECT * FROM generation_requests WHERE id=$1 AND workspace_id=$2",
                        )
                        .bind(id)
                        .bind(workspace)
                        .fetch_optional(&mut *self.tx)
                        .await?;
                        match source {
                            Some(source) => source.visible(self).await?,
                            None => false,
                        }
                    }
                    _ => false,
                };
                if !allowed {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}
