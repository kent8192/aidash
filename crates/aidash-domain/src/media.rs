use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Metadata disclosed with a visible Thread message; attachment bytes are never included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ThreadAttachment {
	pub id: Uuid,
	pub filename: String,
	pub media_type: String,
	pub size_bytes: i64,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub width: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub height: Option<i32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MessageAttachment {
	pub message_id: Uuid,
	pub attachment: ThreadAttachment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
	pub file_id: Uuid,
	pub expected_digest: String,
}

/// One input selected for a model request: a Working Area file, or the page
/// image of one of the Run's Evidence fragments (ADR 0053). A file selection
/// keeps its stored shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ModelMedia {
	File(Selection),
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
