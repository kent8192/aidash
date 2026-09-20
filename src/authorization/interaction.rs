//! Scoped conversation admission and human interaction at the local home node.
use super::{access::Access, catalog, execution, identity::SubjectIdentity, policy::Resource};
use crate::{
    Error, Result, api_schema::ConversationResponse, domain::*, federation::Federation,
    registry::EntityRef,
};
use sea_orm::sea_query::{Alias, Asterisk, Condition, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

impl Access {
    pub(crate) async fn human_visible(&mut self, request: &HumanRequest) -> Result<bool> {
        if let Some(allowed) = self.cached_humans.get(&request.id) {
            return Ok(*allowed);
        }
        let resource = self.human_resource(request).await?;
        let allowed = self.decide(&resource, "human.read").await?;
        self.cached_humans.insert(request.id, allowed);
        Ok(allowed)
    }

    pub(crate) async fn human_reads(&mut self, workspace: Uuid, run: Uuid) -> Result<bool> {
        // Request identity/kind is immutable, but new requests can appear while
        // replaying events. Recheck membership even when decisions are cached.
        let requests: Vec<HumanRequest> = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("human_requests"))
                .cond_where(
                    Condition::all()
                        .add(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
                        .add(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$2"))),
                )
                .to_string(PostgresQueryBuilder),
        )
        .bind(run)
        .bind(workspace)
        .fetch_all(&mut *self.tx)
        .await?;
        for request in requests {
            if !self.human_visible(&request).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) async fn run_for_interaction(&mut self, id: Uuid) -> Result<Run> {
        let run: Run = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("runs"))
                .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .fetch_optional(&mut *self.tx)
        .await?
        .ok_or(Error::Forbidden)?;
        let workspace = self.workspace(run.workspace_id).await?;
        self.context = workspace.attributes.clone();
        self.require(&workspace, "workspace.read").await?;
        if !self.run_visible(&run).await? {
            return Err(Error::Forbidden);
        }
        Ok(run)
    }

    pub(crate) async fn conversation_resource(
        &mut self,
        conversation: &Conversation,
    ) -> Result<Resource> {
        let workspace = self.workspace(conversation.workspace_id).await?;
        let mut attributes = workspace.attributes;
        attributes["created_by"] = json!(conversation.created_by);
        attributes["target"] = json!(conversation.target);
        attributes["target_kind"] = json!(conversation.target_kind);
        Ok(self.resource("conversation", conversation.id, attributes))
    }

    pub(crate) async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
        let workspace = self.workspace(request.workspace_id).await?;
        let mut attributes = workspace.attributes;
        attributes["kind"] = json!(request.kind);
        attributes["run_id"] = json!(request.run_id);
        Ok(self.resource("human_request", request.id, attributes))
    }
}

pub async fn conversation(
    f: &Federation,
    identity: &SubjectIdentity,
    title: &str,
    goal: &str,
    target: &EntityRef,
    target_kind: &str,
) -> Result<ConversationResponse> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = async {
        let workspace = access.create_workspace(&f.store, title, goal).await?;
        let resource = access.workspace(workspace.id).await?;
        access.context = resource.attributes.clone();
        access.require(&resource, "workspace.read").await?;
        let entry = catalog::entry(&mut access, target, "registry.read").await?;
        if entry.kind != target_kind || !matches!(target_kind, "agent" | "cluster") {
            return Err(Error::Invalid(
                "conversation target must be an agent or cluster".into(),
            ));
        }
        let agent = if target_kind == "cluster" {
            catalog::entry(&mut access, target, "cluster.execute").await?;
            serde_json::from_value::<EntityRef>(entry.config["coordinator"].clone()).map_err(
                |_| {
                    Error::Invalid(
                        "cluster requires an explicit coordinator agent reference".into(),
                    )
                },
            )?
        } else {
            target.clone()
        };
        let conversation: Conversation = sqlx::query_as(
            &Query::insert()
                .into_table(Alias::new("conversations"))
                .columns([
                    Alias::new("id"),
                    Alias::new("workspace_id"),
                    Alias::new("target"),
                    Alias::new("target_kind"),
                    Alias::new("created_by"),
                ])
                .values_panic([
                    Expr::cust("$1").into(),
                    Expr::cust("$2").into(),
                    Expr::cust("$3").into(),
                    Expr::cust("$4").into(),
                    Expr::cust("$5").into(),
                ])
                .returning_all()
                .to_string(PostgresQueryBuilder),
        )
        .bind(Uuid::new_v4())
        .bind(workspace.id)
        .bind(format!("{}@{}", target.id, target.version))
        .bind(target_kind)
        .bind(&identity.subject)
        .fetch_one(&mut *access.tx)
        .await?;
        let conversation_resource = access.conversation_resource(&conversation).await?;
        access
            .require(&conversation_resource, "conversation.create")
            .await?;
        access
            .require(&conversation_resource, "conversation.read")
            .await?;
        f.store
            .event(
                &mut access.tx,
                Some(workspace.id),
                "conversation.created",
                json!(conversation),
            )
            .await?;
        access.require(&resource, "message.create").await?;
        f.store
            .message_in(&mut access.tx, workspace.id, &identity.subject, goal, None)
            .await?;
        access.require(&resource, "task.create").await?;
        let task = f
            .store
            .create_task_in(
                &mut access.tx,
                workspace.id,
                &NewTask {
                    title: title.into(),
                    description: goal.into(),
                    requirements: json!({}),
                    dependencies: vec![],
                    parent_id: None,
                },
                &identity.subject,
                None,
            )
            .await?;
        access
            .require(
                &access.resource("task", task.id, json!({"created_by":task.created_by})),
                "task.read",
            )
            .await?;
        let delegation = execution::delegate_in(f, &mut access, task.id, &agent).await?;
        let task = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("tasks"))
                .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                .to_string(PostgresQueryBuilder),
        )
        .bind(task.id)
        .fetch_one(&mut *access.tx)
        .await?;
        Ok(ConversationResponse {
            conversation,
            workspace,
            task,
            delegation,
        })
    }
    .await;
    let response = access.finish(result).await?;
    f.notify.notify_waiters();
    Ok(response)
}

pub async fn message(
    f: &Federation,
    identity: &SubjectIdentity,
    id: Uuid,
    content: &str,
) -> Result<()> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = async {
        let run = access.run_for_interaction(id).await?;
        access
            .require(&access.resource("run", id, json!({})), "run.message")
            .await?;
        access
            .require(
                &access.resource("workspace", run.workspace_id, json!({})),
                "message.create",
            )
            .await?;
        f.store
            .message_in(
                &mut access.tx,
                run.workspace_id,
                &identity.subject,
                content,
                None,
            )
            .await
    }
    .await;
    access.finish(result).await?;
    f.notify.notify_waiters();
    Ok(())
}

pub async fn answer(
    f: &Federation,
    identity: &SubjectIdentity,
    id: Uuid,
    response: Value,
) -> Result<HumanRequest> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = async {
        let request: HumanRequest = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("human_requests"))
                .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .fetch_optional(&mut *access.tx)
        .await?
        .ok_or(Error::Forbidden)?;
        let run = access.run_for_interaction(request.run_id).await?;
        if request.workspace_id != run.workspace_id {
            return Err(Error::Forbidden);
        }
        let resource = access.human_resource(&request).await?;
        access.require(&resource, "human.read").await?;
        access.require(&resource, "human.answer").await?;
        f.store
            .answer_in(&mut access.tx, id, response, &identity.subject)
            .await
    }
    .await;
    let response = access.finish(result).await?;
    f.notify.notify_waiters();
    Ok(response)
}

pub async fn abandon(
    f: &Federation,
    identity: &SubjectIdentity,
    id: Uuid,
    revision: i64,
    reason: &str,
) -> Result<Task> {
    let mut access = Access::begin(&f.store, identity).await?;
    let result = async {
        let task: Task = sqlx::query_as(
            &Query::select()
                .column(Asterisk)
                .from(Alias::new("tasks"))
                .cond_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
                .to_string(PostgresQueryBuilder),
        )
        .bind(id)
        .fetch_optional(&mut *access.tx)
        .await?
        .ok_or(Error::Forbidden)?;
        let workspace = access.workspace(task.workspace_id).await?;
        access.context = workspace.attributes.clone();
        access.require(&workspace, "workspace.read").await?;
        let resource = access.resource("task", id, json!({"created_by":task.created_by}));
        access.require(&resource, "task.read").await?;
        access.require(&resource, "task.abandon").await?;
        f.store
            .abandon_task_in(&mut access.tx, id, revision, reason, &identity.subject)
            .await
    }
    .await;
    let task = access.finish(result).await?;
    f.notify.notify_waiters();
    Ok(task)
}
