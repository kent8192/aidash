//! Management use cases.
use crate::{
	Error, Result,
	apps::federation::remote::serializers::actions::SentResponse,
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
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::management::ThreadInput;
use crate::apps::workspaces::serializers::management::TitleInput;

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
	/// Open a Thread (#1008, ADR 0140): a person's request to an Agent or
	/// cluster opens it with its Root Task; without a target it opens empty.
	pub(crate) async fn thread_create(
		&self,
		actor: Actor,
		input: ThreadInput,
	) -> Result<aidash_domain::threads::ThreadOpened> {
		crate::http::validate(&input)?;
		let f = self.runtime.clone();
		match (&input.target, &input.target_kind) {
			(Some(target), Some(target_kind)) => {
				let request = input.request.as_deref().ok_or_else(|| {
					Error::Invalid("a request to an Agent needs its request text".into())
				})?;
				let Actor::Subject(identity) = actor else {
					return Err(Error::Forbidden);
				};
				Ok(interaction::conversation(
					&f,
					&identity,
					aidash_application::threads::ConversationRequest {
						title: &input.title,
						request,
						target,
						target_kind,
						node: input.node.as_ref(),
						scratch_space_id: input.scratch_space_id,
					},
				)
				.await?
				.into())
			}
			(None, None) => {
				let Some(scope) = scoped(&f, actor) else {
					return Err(Error::Forbidden);
				};
				let thread = scope
					.create(
						&input.title,
						input.node.as_ref(),
						input.scratch_space_id,
						&f.config.scratch_policy,
					)
					.await?;
				if let Some(request) = &input.request {
					scope
						.message_keyed(thread.id, request, Some(Uuid::new_v4()))
						.await?;
				}
				Ok(aidash_domain::threads::ThreadOpened {
					thread,
					conversation: None,
					task: None,
					delegation: None,
				})
			}
			_ => Err(Error::Invalid(
				"target and target_kind are given together".into(),
			)),
		}
	}
	pub(crate) async fn thread_get(&self, actor: Actor, id: Uuid) -> Result<ThreadSnapshot> {
		let f = self.runtime.clone();
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope.snapshot(id).await
	}
	pub(crate) async fn thread_update(
		&self,
		actor: Actor,
		id: Uuid,
		input: TitleInput,
	) -> Result<Thread> {
		let f = self.runtime.clone();
		let Some(scope) = scoped(&f, actor) else {
			return Err(Error::Forbidden);
		};
		scope.update(id, input.revision, &input.title).await
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
				"thread messages do not accept run attachments".into(),
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
}
