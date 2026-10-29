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
		// Tasks are Tenant content: an administrator lists none (#484).
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope.task_page(page.offset).await
	}
	pub(crate) async fn workspace_create(
		&self,
		actor: Actor,
		input: WorkspaceInput,
	) -> Result<Workspace> {
		crate::http::validate(&input)?;
		let f = self.runtime.clone();
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope
			.create(&input.title, &input.goal, input.node.as_ref())
			.await
	}
	pub(crate) async fn workspace_get(&self, actor: Actor, id: Uuid) -> Result<WorkspaceSnapshot> {
		let f = self.runtime.clone();
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope.snapshot(id).await
	}
	pub(crate) async fn workspace_update(
		&self,
		actor: Actor,
		id: Uuid,
		input: StateInput,
	) -> Result<Workspace> {
		let f = self.runtime.clone();
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope.update(id, input.revision, input.state).await
	}
	pub(crate) async fn task_create(
		&self,
		actor: Actor,
		id: Uuid,
		headers: HeaderMap,
		input: NewTask,
	) -> Result<Task> {
		let f = self.runtime.clone();
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope
			.create_task(
				id,
				&input,
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
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope
			.message_keyed(id, &input.content, input.idempotency_key)
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
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		interaction::abandon(&f, &identity, id, input.revision, &input.reason).await
	}
	pub(crate) async fn conversation_create(
		&self,
		actor: Actor,
		input: ConversationInput,
	) -> Result<ConversationResponse> {
		crate::http::validate(&input)?;
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		interaction::conversation(
			&f,
			&identity,
			&input.title,
			&input.goal,
			&input.target,
			&input.target_kind,
			input.node.as_ref(),
		)
		.await
	}
}
