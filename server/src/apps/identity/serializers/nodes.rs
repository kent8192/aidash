//! Contracts of a Tenant's Nodes (#241).
use chrono::{DateTime, Utc};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A Node as its Tenant sees it.
#[derive(Serialize, JsonSchema)]
pub(crate) struct NodeRecord {
	/// The Node name Aidash assigned. It never changes.
	pub(crate) name: String,
	/// `aidash://<deployment>/<name>`, what other Nodes register.
	pub(crate) address: String,
	pub(crate) label: String,
	/// The current default Node; Workspaces go there unless another is chosen.
	pub(crate) default: bool,
	pub(crate) created_at: DateTime<Utc>,
	pub(crate) state: NodeState,
	pub(crate) deletion: Option<NodeDeletionRecord>,
	/// Retained data is physically removed by the purge worker. Offering and
	/// Tenant deletion rules determine whether retained bytes are billable.
	pub(crate) storage_retained_until_purge: bool,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeCreation {
	/// The Node label: 1 to 64 characters after trimming, without control characters.
	#[validate(length(min = 1, max = 64))]
	pub(crate) label: String,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeLabelUpdate {
	#[validate(length(min = 1, max = 64))]
	pub(crate) label: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NodeState {
	Live,
	Deleted,
	Deleting,
	Purged,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct NodeDeletionRecord {
	pub(crate) id: uuid::Uuid,
	pub(crate) deleted_at: DateTime<Utc>,
	pub(crate) deleted_by_kind: String,
	pub(crate) purge_after: DateTime<Utc>,
	pub(crate) purged_at: Option<DateTime<Utc>>,
	pub(crate) purge_step: Option<String>,
	pub(crate) purge_failures: i64,
	pub(crate) purge_error: Option<String>,
	pub(crate) purge_failed_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeDeletionRequest {
	pub(crate) confirmation: String,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeList {
	#[serde(default)]
	pub(crate) include_deleted: bool,
	/// Page size for deletion management; ordinary selectors return all live Nodes.
	#[serde(default = "node_page_size")]
	#[validate(range(min = 1, max = 100))]
	pub(crate) limit: u64,
	#[serde(default)]
	/// Offset for deletion management.
	pub(crate) offset: u64,
}

fn node_page_size() -> u64 {
	100
}
