use serde::{Deserialize, Serialize};
// Serializable contracts for authorization.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;

#[derive(Deserialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	pub(crate) grant_id: Uuid,
}

#[derive(Serialize, Deserialize, JsonSchema, reinhardt::Validate)]
pub(crate) struct Admission {
	pub(crate) id: Uuid,
	pub(crate) source_node: String,
	pub(crate) grant_id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) expires_at: DateTime<Utc>,
}

// Serializable peer admission contracts.

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteExecutionControlInput {
	pub(crate) grant_id: Uuid,
	pub(crate) action: crate::authorization::remote::execution::RemoteExecutionControl,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageInput {
	pub(crate) grant_id: Uuid,
	pub(crate) message: crate::authorization::remote::execution::RemoteExecutionMessageInput,
}

use uuid::Uuid;
