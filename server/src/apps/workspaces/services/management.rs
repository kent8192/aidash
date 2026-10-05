//! Management use cases.
use crate::{
	Error, Result,
	apps::federation::remote::serializers::actions::SentResponse,
	apps::workspaces::serializers::tasks::ConversationResponse,
	apps::workspaces::serializers::tasks::PageQuery,
	apps::workspaces::serializers::tasks::TaskPage,
	authorization::{identity::Actor, interaction},
	domain::*,
	federation::Federation,
};
use http::HeaderMap;
use reinhardt::injectable;
use uuid::Uuid;

use crate::apps::workspaces::serializers::management::AbandonInput;
use crate::apps::workspaces::serializers::management::ConversationInput;
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::management::StateInput;
use crate::apps::workspaces::serializers::management::WorkspaceInput;

use crate::apps::identity::services::http_auth::scoped;

#[derive(Clone)]
pub struct CollaborationManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> CollaborationManagement {
	CollaborationManagement { runtime }
}
impl CollaborationManagement {
	pub(crate) async fn task_list(&self, actor: Actor, page: PageQuery) -> Result<TaskPage> {
		let f = self.runtime.clone();
		if let Some(scope) = scoped(&f, actor) {
			return scope.task_page(page.offset).await;
		}
		let mut page = f.store.task_page(page.offset).await?;
		let denied = crate::authorization::remote::operator::blocked(
			&mut crate::database::native::begin(&f.store.pool).await?,
			&page
				.tasks
				.iter()
				.map(|task| task.workspace_id)
				.collect::<Vec<_>>(),
		)
		.await?;
		page.tasks
			.retain(|task| !denied.contains(&task.workspace_id));
		Ok(page)
	}
	pub(crate) async fn workspace_create(
		&self,
		actor: Actor,
		input: WorkspaceInput,
	) -> Result<Workspace> {
		crate::http::validate(&input)?;
		let f = self.runtime.clone();
		if let Some(scope) = scoped(&f, actor) {
			return scope.create(&input.title, &input.goal).await;
		}
		f.store.create_workspace(&input.title, &input.goal).await
	}
	pub(crate) async fn workspace_get(&self, actor: Actor, id: Uuid) -> Result<WorkspaceSnapshot> {
		let f = self.runtime.clone();
		if let Some(scope) = scoped(&f, actor) {
			return scope.snapshot(id).await;
		}
		crate::authorization::remote::operator::require(
			&mut crate::database::native::begin(&f.store.pool).await?,
			id,
		)
		.await?;
		f.store.snapshot(id).await
	}
	pub(crate) async fn workspace_update(
		&self,
		actor: Actor,
		id: Uuid,
		input: StateInput,
	) -> Result<Workspace> {
		let f = self.runtime.clone();
		if let Some(scope) = scoped(&f, actor) {
			return scope.update(id, input.revision, input.state).await;
		}
		crate::authorization::remote::operator::require(
			&mut crate::database::native::begin(&f.store.pool).await?,
			id,
		)
		.await?;
		f.store.update_state(id, input.revision, input.state).await
	}
	pub(crate) async fn task_create(
		&self,
		actor: Actor,
		id: Uuid,
		headers: HeaderMap,
		input: NewTask,
	) -> Result<Task> {
		let f = self.runtime.clone();
		if let Some(scope) = scoped(&f, actor) {
			return scope
				.create_task(
					id,
					&input,
					headers.get("idempotency-key").and_then(|h| h.to_str().ok()),
				)
				.await;
		}
		f.store
			.create_task(
				id,
				&input,
				"human",
				headers.get("idempotency-key").and_then(|h| h.to_str().ok()),
			)
			.await
	}
	pub(crate) async fn message_create(
		&self,
		actor: Actor,
		id: Uuid,
		input: MessageInput,
	) -> Result<SentResponse> {
		if !input.attachment_ids.is_empty() {
			return Err(Error::Invalid(
				"workspace messages do not accept run attachments".into(),
			));
		}
		let f = self.runtime.clone();
		if let Some(scope) = scoped(&f, actor) {
			scope
				.message_keyed(id, &input.content, input.idempotency_key)
				.await?;
			return Ok(SentResponse { sent: true });
		}
		let key = format!(
			"workspace-human:{id}:{}",
			input.idempotency_key.unwrap_or_else(Uuid::new_v4)
		);
		f.store
			.message(id, "human", &input.content, Some(&key))
			.await?;
		Ok(SentResponse { sent: true })
	}
	pub(crate) async fn task_abandon(
		&self,
		actor: Actor,
		id: Uuid,
		input: AbandonInput,
	) -> Result<Task> {
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return interaction::abandon(&f, &identity, id, input.revision, &input.reason).await;
		}
		let existing = f.store.task(id).await?;
		crate::authorization::remote::operator::require(
			&mut crate::database::native::begin(&f.store.pool).await?,
			existing.workspace_id,
		)
		.await?;
		let task = f
			.store
			.abandon_task(id, input.revision, &input.reason)
			.await?;
		f.notify.notify_waiters();
		Ok(task)
	}
	pub(crate) async fn conversation_create(
		&self,
		actor: Actor,
		input: ConversationInput,
	) -> Result<ConversationResponse> {
		crate::http::validate(&input)?;
		let f = self.runtime.clone();
		if let Actor::Subject(identity) = actor {
			return interaction::conversation(
				&f,
				&identity,
				&input.title,
				&input.goal,
				&input.target,
				&input.target_kind,
			)
			.await;
		}
		aidash_application::workspaces::operator_conversation(
			&crate::bootstrap::operator_conversations(&f),
			aidash_application::workspaces::ConversationRequest {
				title: &input.title,
				goal: &input.goal,
				target: &input.target,
				target_kind: &input.target_kind,
			},
		)
		.await
		.map_err(Into::into)
	}
}
