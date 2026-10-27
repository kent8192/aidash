//! HTTP and workers adapt DTOs to one shared approval and authorization workflow.
use super::{contracts::Area, records::Record};
pub use crate::apps::execution::capabilities::serializers::approvals::{
	ApprovalDecision, Outbound, Revoke,
};
use crate::{Error, Result, authorization::access::Access, domain::Run, store::Store};
use aidash_application::ports::capabilities::approvals::ApprovalScope as _;
use serde_json::Value;
use uuid::Uuid;
pub(crate) async fn can_allow_always(
	store: &Store,
	access: &mut Access,
	record: &Record,
) -> Result<bool> {
	let mut scope = crate::bootstrap::approval_scope(Some(store), access, None);
	let run = scope
		.run_context(serde_json::from_value(record.data["run_id"].clone())?)
		.await?;
	aidash_application::capabilities::approvals::can_allow_always(
		&mut scope,
		&run,
		&crate::apps::execution::repositories::capability_records::domain(record.clone()),
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn visible(access: &mut Access, record: &Record) -> Result<bool> {
	let mut scope = crate::bootstrap::approval_scope(None, access, None);
	aidash_application::capabilities::approvals::visible(
		&mut scope,
		&crate::apps::execution::repositories::capability_records::domain(record.clone()),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: Outbound,
) -> Result<Value> {
	let mut scope = crate::bootstrap::approval_scope(Some(store), access, Some(area));
	scope.run = Some(Box::new(run.clone()));
	aidash_application::capabilities::approvals::prepare(
		&mut scope,
		&run.metadata(),
		area.id,
		input.into(),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn view(
	store: &Store,
	access: &mut Access,
	run: &Run,
	record: &Record,
) -> Result<Value> {
	let mut scope = crate::bootstrap::approval_scope(Some(store), access, None);
	aidash_application::capabilities::approvals::view(
		&mut scope,
		&run.metadata(),
		&crate::apps::execution::repositories::capability_records::domain(record.clone()),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn decide(
	store: &Store,
	access: &mut Access,
	id: Uuid,
	input: ApprovalDecision,
) -> Result<Value> {
	let human = match input.choice {
		crate::apps::execution::capabilities::serializers::approvals::ApprovalChoice::Deny => {
			aidash_domain::decision::observation::HumanLabel::Deny
		}
		_ => aidash_domain::decision::observation::HumanLabel::Allow,
	};
	let mut scope = crate::bootstrap::approval_scope(Some(store), access, None);
	let result =
		aidash_application::capabilities::approvals::decide(&mut scope, id, input.into()).await?;
	if result["state"] == "approved" || result["state"] == "denied" {
		crate::apps::execution::observation::label(
			&mut *access.tx,
			&access.identity.tenant,
			id,
			human,
		)
		.await?;
	}
	Ok(result)
}
/// The kind of the approval record `id` names, among those a revocation can
/// end: an outbound read and a Tool call approval.
pub(crate) async fn revocable_kind(access: &mut Access, id: Uuid) -> Result<&'static str> {
	for kind in [
		"outbound",
		aidash_application::capabilities::approvals::TOOL_CALL,
		aidash_application::capabilities::approvals::SAMPLING,
	] {
		match super::records::get(access, id, kind).await {
			Ok(_) => return Ok(kind),
			Err(Error::NotFound(_)) => {}
			Err(error) => return Err(error),
		}
	}
	Err(Error::NotFound("grant unavailable".into()))
}
pub(crate) async fn revoke(
	access: &mut Access,
	id: Uuid,
	kind: &str,
	input: Revoke,
) -> Result<Value> {
	let mut scope = crate::bootstrap::approval_scope(None, access, None);
	aidash_application::capabilities::approvals::revoke(&mut scope, id, kind, input.into())
		.await
		.map_err(Into::into)
}
pub(crate) async fn authorize(
	store: &Store,
	access: &mut Access,
	record: &Record,
) -> Result<aidash_domain::RunMetadata> {
	let mut scope = crate::bootstrap::approval_scope(Some(store), access, None);
	aidash_application::capabilities::approvals::authorize(
		&mut scope,
		&crate::apps::execution::repositories::capability_records::domain(record.clone()),
	)
	.await
	.map_err(Into::into)
}
