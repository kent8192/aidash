//! Local context classification reserves one durable attempt for every ancestor.
use crate::registry::EntityRef;
use uuid::Uuid;

pub struct Context {
	pub run: Uuid,
	pub task: Uuid,
	pub agent: EntityRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
	pub id: Uuid,
	pub run: Uuid,
	pub provider: EntityRef,
	pub request_bytes: i64,
	pub questions: i32,
}
