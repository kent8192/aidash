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
	pub skill_files: usize,
	pub skill_bytes: usize,
	pub reference_files: usize,
	pub reference_bytes: u64,
	pub reference_pages: usize,
	pub reference_text_bytes: usize,
	pub share_files: usize,
	pub share_file_bytes: u64,
	pub share_bytes: u64,
}
