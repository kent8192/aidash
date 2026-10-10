//! Generation service adapters compose the shared application use cases.
use crate::{
	Result,
	authorization::{access::Access, identity::SubjectIdentity, policy::Resource},
	federation::Federation,
};
use serde_json::json;
use uuid::Uuid;
#[path = "budget.rs"]
pub mod budget;
#[path = "compaction.rs"]
pub mod compaction;
#[path = "embedding.rs"]
pub mod embedding;
#[path = "foreign.rs"]
pub(crate) mod foreign;
#[path = "lifecycle.rs"]
pub mod lifecycle;
#[path = "policy.rs"]
pub mod policy;
#[path = "provision.rs"]
pub mod provision;
#[path = "remote.rs"]
pub(crate) mod remote;
pub use crate::apps::execution::generation::serializers::contracts::{Assignment, Request};
pub(crate) fn resource(access: &Access, id: &str) -> Resource {
	access.resource("generation_policy", id, json!({}))
}
pub async fn assign(
	f: &Federation,
	identity: &SubjectIdentity,
	task_id: Uuid,
	policy_id: &str,
	reason: &str,
) -> Result<Assignment> {
	aidash_application::generation::assignment::assign(
		&crate::bootstrap::generation_assignment_repository(f, identity),
		task_id,
		policy_id,
		reason,
		&crate::bootstrap::registry_validation_for(&f.store),
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn assign_in(
	f: &Federation,
	access: &mut Access,
	task_id: Uuid,
	policy_id: &str,
	reason: &str,
) -> Result<Assignment> {
	aidash_application::generation::assignment::assign_in(
		&mut crate::bootstrap::generation_assignment_scope(f, access),
		task_id,
		policy_id,
		reason,
		&crate::bootstrap::registry_validation_for(&f.store),
	)
	.await
	.map_err(Into::into)
}
