//! Management API contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct WorkspaceCommand {
	pub(crate) task_id: Uuid,
	pub(crate) agent: EntityRef,
	pub(crate) operation: String,
	pub(crate) data: Value,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct RemoteControl {
	pub(crate) run_id: Uuid,
	pub(crate) action: String,
	pub(crate) request_id: Option<Uuid>,
	pub(crate) response: Option<Value>,
	pub(crate) content: Option<String>,
	pub(crate) idempotency_key: Option<Uuid>,
}
