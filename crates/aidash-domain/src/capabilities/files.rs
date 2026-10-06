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
