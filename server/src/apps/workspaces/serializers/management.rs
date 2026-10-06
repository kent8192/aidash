//! Management API contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Deserialize, Serialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceInput {
	#[serde(deserialize_with = "crate::apps::workspaces::services::validation::trimmed_text")]
	#[validate(length(min = 1))]
	pub(crate) title: String,
	#[serde(deserialize_with = "crate::apps::workspaces::services::validation::trimmed_text")]
	#[validate(length(min = 1))]
	pub(crate) goal: String,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StateInput {
	pub(crate) revision: i64,
	pub(crate) state: Value,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageInput {
	#[serde(default)]
	pub(crate) attachment_ids: Vec<Uuid>,
	pub(crate) content: String,
	pub(crate) idempotency_key: Option<Uuid>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct AbandonInput {
	pub(crate) revision: i64,
	pub(crate) reason: String,
}
#[derive(Deserialize, Serialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationInput {
	#[serde(deserialize_with = "crate::apps::workspaces::services::validation::trimmed_text")]
	#[validate(length(min = 1))]
	pub(crate) title: String,
	#[serde(deserialize_with = "crate::apps::workspaces::services::validation::trimmed_text")]
	#[validate(length(min = 1))]
	pub(crate) goal: String,
	pub(crate) target: EntityRef,
	pub(crate) target_kind: String,
}
