//! Management API contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opening a Thread (#1008, ADR 0140). With a `target`, the `request` is
/// a person's request to that Agent or cluster: it becomes the first message
/// and the description of the Root Task delegated to the target. Without a
/// target the Thread opens empty, and a `request` is refused.
#[derive(Deserialize, Serialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThreadInput {
	#[serde(deserialize_with = "crate::apps::workspaces::services::validation::trimmed_text")]
	#[validate(length(min = 1))]
	pub(crate) title: String,
	#[serde(
		default,
		deserialize_with = "crate::apps::workspaces::services::validation::optional_trimmed_text"
	)]
	pub(crate) request: Option<String>,
	#[schemars(length(max = 4000))]
	pub(crate) goal: Option<String>,
	#[serde(default)]
	pub(crate) target: Option<EntityRef>,
	#[serde(default)]
	pub(crate) target_kind: Option<String>,
	/// The Node the Thread belongs to. The Tenant's default Node when absent.
	#[serde(default)]
	pub(crate) node: Option<aidash_domain::nodes::NodeName>,
	#[serde(default)]
	pub(crate) scratch_space_id: Option<Uuid>,
}
/// Renaming a Thread: its title is the only attribute that changes.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TitleInput {
	pub(crate) revision: i64,
	pub(crate) title: String,
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
