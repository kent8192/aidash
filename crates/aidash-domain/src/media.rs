use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
	pub file_id: Uuid,
	pub expected_digest: String,
}

/// Immutable metadata used to select a file for one model request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedFile {
	pub file_id: Uuid,
	pub path: String,
	pub digest: String,
	pub size: u64,
	pub media_type: String,
}
