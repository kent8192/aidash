//! HTTP and workers adapt DTOs to one shared approval and authorization workflow.
use super::{contracts::Area, records::Record};
pub use crate::apps::execution::capabilities::serializers::approvals::{
	ApprovalDecision, Outbound, Revoke,
};
use crate::{Result, authorization::access::Access, domain::Run, store::Store};
use serde_json::Value;
use uuid::Uuid;
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
	let mut scope = crate::bootstrap::approval_scope(Some(store), access, None);
	aidash_application::capabilities::approvals::decide(&mut scope, id, input.into())
		.await
		.map_err(Into::into)
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
