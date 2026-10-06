//! Explicit Home generation intents and prepared foreign-task executors. No
//! local Task or Run is fabricated to reuse local generation privileges.
use crate::{
	Result,
	authorization::{access::Access, identity::Actor},
	domain::Task,
	federation::Federation,
	registry::EntityRef,
};

use serde_json::Value;
use uuid::Uuid;

pub(crate) use aidash_domain::generation::intent::Intent;
pub(crate) use aidash_domain::generation::intent::{Input, Prepared, Reference};

pub(crate) async fn request(
	f: Federation,
	actor: Actor,
	task_id: Uuid,
	input: Input,
) -> Result<Prepared> {
	aidash_application::generation::foreign::home::request(
		&crate::bootstrap::generation_home_repository(&f, actor),
		task_id,
		input,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn describe(
	f: Federation,
	headers: HeaderMap,
	input: Reference,
) -> Result<Intent> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	aidash_application::generation::foreign::home::describe(
		&crate::bootstrap::generation_home_repository(&f, Actor::Operator),
		source,
		input.intent_id,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn prepare(
	f: Federation,
	headers: HeaderMap,
	input: Reference,
) -> Result<Prepared> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	prepare_at(&f, source, input.intent_id).await
}
async fn prepare_at(f: &Federation, source: &str, id: Uuid) -> Result<Prepared> {
	aidash_application::generation::foreign::receiver::prepare(
		&crate::bootstrap::generation_receiver_repository(f),
		source,
		id,
	)
	.await
	.map_err(Into::into)
}

/// Called by the receiver's inspection leaf under current mapped authority.
pub(crate) async fn inspect(
	access: &mut Access,
	source: &str,
	task: Option<Uuid>,
	agent: &EntityRef,
) -> Result<Option<Value>> {
	aidash_application::generation::foreign::inspect(
		&mut crate::bootstrap::generation_foreign_guard(access),
		source,
		task,
		agent,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn bind(
	f: &Federation,
	access: &mut Access,
	description: &crate::authorization::remote::Description,
	admission: Uuid,
	activate: bool,
) -> Result<()> {
	aidash_application::generation::foreign::bind(
		&mut crate::bootstrap::generation_foreign_binding(access, f),
		description,
		admission,
		activate,
	)
	.await
	.map_err(Into::into)
}
/// Home validates the saved intent locally, without a nested callback to B.
pub(crate) async fn check_home(
	access: &mut Access,
	task: &Task,
	node: &str,
	generation: Option<&Value>,
) -> Result<()> {
	aidash_application::generation::foreign::check_home(
		&mut crate::bootstrap::generation_foreign_guard(access),
		task,
		node,
		generation,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn require_active(
	access: &mut Access,
	description: &crate::authorization::remote::Description,
	run: Uuid,
) -> Result<()> {
	aidash_application::generation::foreign::require_active(
		&mut crate::bootstrap::generation_foreign_guard(access),
		description,
		run,
	)
	.await
	.map_err(Into::into)
}

use http::HeaderMap;
pub(crate) async fn cancel(f: Federation, actor: Actor, (task, id): (Uuid, Uuid)) -> Result<bool> {
	aidash_application::generation::foreign::home::cancel(
		&crate::bootstrap::generation_home_repository(&f, actor),
		&crate::bootstrap::generation_foreign_maintenance(&f),
		task,
		id,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn cancel_at(f: Federation, headers: HeaderMap, input: Reference) -> Result<bool> {
	let source = crate::apps::identity::services::http_auth::peer_node(&headers)?;
	aidash_application::generation::foreign::maintenance::cancel_at(
		&crate::bootstrap::generation_foreign_maintenance(&f),
		source,
		input.intent_id,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn reconcile(f: &Federation) -> Result<()> {
	aidash_application::generation::foreign::maintenance::reconcile(
		&crate::bootstrap::generation_foreign_maintenance(f),
	)
	.await
	.map_err(Into::into)
}
