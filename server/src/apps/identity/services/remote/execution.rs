//! Source-side activation and command journal for subject-scoped remote work.
//! Receiver admissions never enter the legacy offer/workspace authority path.
#[path = "execution/commands.rs"]
pub(crate) mod commands;
use super::*;
pub(crate) use crate::apps::identity::repositories::remote_grants::HomeBinding;
pub use aidash_domain::federation::execution::home::FollowUpInput;

pub(crate) async fn binding(access: &mut Access, grant: Uuid) -> Result<Option<HomeBinding>> {
	crate::apps::identity::repositories::home_execution::binding(access, grant).await
}

pub(crate) async fn message(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
	input: RemoteExecutionMessageInput,
) -> Result<RemoteExecutionMessageReceipt> {
	aidash_application::authorization::home::message(
		&crate::bootstrap::home_execution_repository(&f, actor),
		task,
		id,
		input.into(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(crate) async fn control(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
	input: RemoteExecutionControlInput,
) -> Result<RemoteExecutionActivation> {
	aidash_application::authorization::home::control(
		&crate::bootstrap::home_execution_repository(&f, actor),
		task,
		id,
		input.action,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(crate) async fn list(
	f: Federation,
	actor: Actor,
	task: Uuid,
) -> Result<Vec<RemoteExecutionStatus>> {
	aidash_application::authorization::home::list(
		&crate::bootstrap::home_execution_repository(&f, actor),
		task,
	)
	.await
	.map(|r| r.into_iter().map(Into::into).collect())
	.map_err(Into::into)
}

pub(crate) async fn delegate(
	f: &Federation,
	identity: &super::super::identity::SubjectIdentity,
	task: Uuid,
	node: &str,
	agent: &EntityRef,
) -> Result<crate::federation::Delegation> {
	aidash_application::authorization::home::delegate(
		&crate::bootstrap::home_execution_repository(f, Actor::Subject(identity.clone())),
		task,
		node,
		agent,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn activate(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
) -> Result<RemoteExecutionActivation> {
	aidash_application::authorization::home::activate(
		&crate::bootstrap::home_execution_repository(&f, actor),
		task,
		id,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

/// The destination must verify this binding before creating its local Run.
pub(crate) async fn activation_binding(
	f: Federation,
	headers: HeaderMap,
	input: VerifyInput,
) -> Result<Uuid> {
	let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	aidash_application::authorization::home::activation_binding(
		&crate::bootstrap::home_execution_repository(&f, Actor::Operator),
		node,
		input.grant_id,
	)
	.await
	.map_err(Into::into)
}

use http::HeaderMap;

#[derive(Clone)]
pub struct RemoteExecutionManagement {
	pub(crate) runtime: Federation,
}
// Preserve a statement before the value until reinhardt-web#6441 is fixed.
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RemoteExecutionManagement {
	tracing::trace!(
		service = "RemoteExecutionManagement",
		"creating injectable service"
	);
	RemoteExecutionManagement { runtime }
}

use reinhardt::injectable;

pub use crate::apps::identity::serializers::remote_execution::{
	RemoteExecutionActivation, RemoteExecutionControl, RemoteExecutionControlInput,
	RemoteExecutionMessageInput, RemoteExecutionMessageReceipt, RemoteExecutionStatus,
};

pub(crate) async fn follow_up(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
	input: FollowUpInput,
) -> Result<Task> {
	aidash_application::authorization::home::follow_up(
		&crate::bootstrap::home_execution_repository(&f, actor),
		task,
		id,
		input,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn provenance(
	f: Federation,
	actor: Actor,
	(task, id): (Uuid, Uuid),
) -> Result<Option<crate::semantic::remote::status::Provenance>> {
	aidash_application::authorization::home::provenance(
		&crate::bootstrap::home_execution_repository(&f, actor),
		task,
		id,
	)
	.await
	.map_err(Into::into)
}

pub use aidash_domain::federation::execution::admission::{
	RemoteExecutionControlState, RemoteExecutionPhase,
};
