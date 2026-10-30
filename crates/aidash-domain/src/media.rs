use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
	WorkingImage(WorkingImageSelection),
	PageImage(PageImageSelection),
}

impl ModelMedia {
	/// Working Image opening degrades to metadata when the selected model
	/// cannot accept images. Other selections retain their existing behavior.
	pub fn supported_by(&self, modalities: &[String]) -> bool {
		!matches!(self, Self::WorkingImage(_)) || modalities.iter().any(|item| item == "image")
	}
}

/// A current Workspace image selected by its Area and immutable File version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingImageSelection {
	pub area_id: Uuid,
	pub generation: i64,
	pub file_id: Uuid,
	pub path: String,
	pub expected_digest: String,
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

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn working_image_needs_image_modality() {
		let image = ModelMedia::WorkingImage(WorkingImageSelection {
			area_id: Uuid::nil(),
			generation: 1,
			file_id: Uuid::nil(),
			path: "chart.png".into(),
			expected_digest: "digest".into(),
		});
		assert!(!image.supported_by(&["text".into()]));
		assert!(image.supported_by(&["text".into(), "image".into()]));
	}
}
