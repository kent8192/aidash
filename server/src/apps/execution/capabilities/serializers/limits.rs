use serde::{Deserialize, Serialize};
// Serializable limits contracts.

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(default, deny_unknown_fields)]
pub struct ContentLimits {
	pub command_bytes: usize,
	pub patch_bytes: usize,
	pub search_matches: usize,
	pub search_bytes: usize,
	pub search_seconds: u64,
	pub read_bytes: usize,
	// Accept legacy Deployment profiles without publishing or enforcing the removed limits.
	#[serde(rename = "skill_files", skip_serializing)]
	#[schemars(skip)]
	pub(crate) _legacy_skill_files: Option<usize>,
	#[serde(rename = "skill_bytes", skip_serializing)]
	#[schemars(skip)]
	pub(crate) _legacy_skill_bytes: Option<usize>,
	pub reference_files: usize,
	pub reference_bytes: u64,
	pub reference_pages: usize,
	/// The text of a whole Reference set.
	pub reference_text_bytes: usize,
	/// The extracted text of one PDF or Excel file.
	pub reference_file_text_bytes: usize,
	pub share_files: usize,
	pub share_file_bytes: u64,
	pub share_bytes: u64,
}
