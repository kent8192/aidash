//! Summary Stage calls reserve one durable attempt for every generated ancestor.
use crate::registry::EntityRef;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
	pub id: Uuid,
	pub run: Uuid,
	pub provider: EntityRef,
	pub definition_digest: String,
	pub request_bytes: i64,
}

/// Run identity used to check generated-ancestor Summary authority.
pub struct Context {
	pub run: Uuid,
	pub task: Uuid,
	pub agent: EntityRef,
}
