//! File selections, cursors and envelopes contain no filesystem or HTTP implementation.
use super::operations::FileScope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
	Literal,
	Regex,
	Path,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSearch {
	pub query: String,
	pub mode: SearchMode,
	pub scope: FileScope,
	pub path: Option<String>,
	pub cursor: Option<String>,
	pub limit: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
	Text,
	Metadata,
	ModelInput,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRead {
	pub file_id: Uuid,
	pub representation: Representation,
	pub offset: Option<usize>,
	pub max_bytes: Option<usize>,
	pub expected_digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Materialize {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub source: MaterializeSource,
	pub path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterializeSource {
	File {
		file_id: Uuid,
		expected_digest: String,
	},
	Message {
		message_id: Uuid,
	},
	ReferenceText {
		index: usize,
	},
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
	pub area: uuid::Uuid,
	pub generation: i64,
	pub revision: i64,
	pub query: String,
	pub file: usize,
	pub line: usize,
}
#[derive(Debug)]
pub struct Envelope {
	pub operation_id: String,
	pub status: String,
	pub policy_revision: i64,
	pub area_id: Uuid,
	pub generation: i64,
	pub revision: i64,
	pub result: Value,
}
/// The Scratch Space, as a `file_search` scope (#745).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScratchScope {
	Scratch,
}
/// A `file_read` of a path in the Run's Scratch Space (#745). Paths are relative
/// to the Scratch Space root.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchRead {
	pub path: String,
	pub representation: Option<Representation>,
	pub expected_digest: Option<String>,
	pub offset: Option<u64>,
	pub max_bytes: Option<usize>,
}
/// A `file_search` of the Run's Scratch Space (#745). `path` narrows it to a
/// directory or file and `glob` to matching paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchSearch {
	pub query: String,
	pub mode: SearchMode,
	pub scope: ScratchScope,
	pub path: Option<String>,
	pub glob: Option<String>,
	pub limit: Option<usize>,
}
/// Register Scratch Space files in the Working Area (#745). Each of `paths` is
/// a file or a directory; `include` beats `exclude`, which beats the rules of
/// `ignore_files`. With `sync`, registered files under `paths` that the
/// selection no longer holds are removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchSave {
	pub idempotency_key: Uuid,
	pub paths: Vec<String>,
	#[serde(default = "default_ignore_files")]
	pub ignore_files: Vec<String>,
	#[serde(default)]
	pub exclude: Vec<String>,
	#[serde(default)]
	pub include: Vec<String>,
	#[serde(default)]
	pub sync: bool,
	pub expected_revision: Option<i64>,
}
pub fn default_ignore_files() -> Vec<String> {
	vec![".gitignore".into()]
}
/// Remove registered files, or every file under a directory, from the Working Area.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingRemove {
	pub idempotency_key: Uuid,
	pub paths: Vec<String>,
	pub expected_revision: Option<i64>,
}
/// Copy registered Working Area files into the Scratch Space at the same
/// paths, or under `destination` (#745).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingCopy {
	pub idempotency_key: Uuid,
	pub paths: Vec<String>,
	pub destination: Option<String>,
}
