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
	/// The Image Description its writer gave an image, if any (#622).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AttachmentRecord {
	pub message_id: Uuid,
	pub attachment: ThreadAttachment,
	pub sha256: String,
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
	WorkingImage(WorkingImageSelection),
	PageImage(PageImageSelection),
	ThreadAttachment(ThreadAttachmentSelection),
}

/// An immutable attachment selected under the Run's current message authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadAttachmentSelection {
	pub attachment_id: Uuid,
	pub message_id: Uuid,
	pub expected_digest: String,
	#[serde(default)]
	pub wav: bool,
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

/// The selection with which this Run's own `workspace_read` put Thread
/// Attachment `attachment` into its inference, if its history still shows one
/// for `digest`. Only such an attachment may be an image query (#622 decision
/// 4). A selection refused for lack of headroom was recorded as an error and
/// does not count; one a compaction removed must be selected again.
pub fn selected_attachment(
	history: &[crate::context::HistoryEntry],
	attachment: Uuid,
	digest: &str,
) -> Option<ThreadAttachmentSelection> {
	history.iter().rev().find_map(|entry| {
		let crate::context::ContextEvent::Tool { call, result } = &entry.event else {
			return None;
		};
		if call.name != "workspace_read"
			|| call.arguments["kind"] != "attachment"
			|| call.arguments["representation"] != "model_input"
			|| result["status"] != "completed"
		{
			return None;
		}
		let selection: ThreadAttachmentSelection =
			serde_json::from_value(result["selection"].clone()).ok()?;
		(selection.attachment_id == attachment
			&& selection.expected_digest == digest
			&& call.arguments["id"] == attachment.to_string())
		.then_some(selection)
	})
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

	fn tool(
		arguments: serde_json::Value,
		result: serde_json::Value,
	) -> crate::context::HistoryEntry {
		crate::context::HistoryEntry {
			seq: 1,
			event: crate::context::ContextEvent::Tool {
				call: crate::provider::ToolCall {
					id: "c".into(),
					name: "workspace_read".into(),
					arguments,
				},
				result,
			},
			research: None,
		}
	}

	#[test]
	fn only_a_completed_selection_of_the_same_bytes_counts_as_seen() {
		// Arrange
		let attachment = Uuid::from_u128(7);
		let message = Uuid::from_u128(8);
		let input =
			serde_json::json!({"kind":"attachment","id":attachment,"representation":"model_input"});
		let selected = serde_json::json!({"status":"completed","metadata":{"id":attachment},
			"selection":{"attachment_id":attachment,"message_id":message,"expected_digest":"d1","wav":false}});
		let metadata_only = serde_json::json!({"kind":"attachment","id":attachment});
		let refused =
			serde_json::json!({"status":"error","error":"model media input exceeds count limit"});

		// Act
		let seen = selected_attachment(&[tool(input.clone(), selected.clone())], attachment, "d1");
		let other_bytes =
			selected_attachment(&[tool(input.clone(), selected.clone())], attachment, "d2");
		let read_only = selected_attachment(
			&[tool(metadata_only, json_completed(attachment))],
			attachment,
			"d1",
		);
		let failed = selected_attachment(&[tool(input, refused)], attachment, "d1");

		// Assert
		assert_eq!(seen.map(|s| s.message_id), Some(message));
		assert!(other_bytes.is_none());
		assert!(read_only.is_none());
		assert!(failed.is_none());
	}

	fn json_completed(attachment: Uuid) -> serde_json::Value {
		serde_json::json!({"status":"completed","metadata":{"id":attachment}})
	}
}
