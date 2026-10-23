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
	/// The Node the Tenant started with; Workspaces go there unless another is chosen.
	pub(crate) default: bool,
	pub(crate) created_at: DateTime<Utc>,
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
