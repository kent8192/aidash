use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
	pub file_id: Uuid,
	pub expected_digest: String,
}

/// A one-inference file reference can additionally name a creator-owned Subnode Area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelFileSelection {
	pub file_id: Uuid,
	pub expected_digest: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub workspace_id: Option<Uuid>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub area_id: Option<Uuid>,
}

/// One input selected for a model request: a Working Area file, or the page
/// image of one of the Run's Evidence fragments (ADR 0053). A file selection
/// keeps its stored shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ModelMedia {
	File(ModelFileSelection),
	PageImage(PageImageSelection),
}

/// The page image of an Evidence fragment, by the digest the model was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageImageSelection {
	pub evidence_ref: String,
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
