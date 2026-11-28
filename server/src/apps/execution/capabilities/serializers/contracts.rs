use serde::{Deserialize, Serialize};
// Serializable contracts contracts.
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileScope {
	Working,
	References,
	Received,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
	Literal,
	Regex,
	Path,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct FileSearch {
	/// May be empty in `path` mode, which then lists every path in the scope.
	#[validate(length(max = 1024))]
	pub query: String,
	pub mode: SearchMode,
	pub scope: FileScope,
	pub path: Option<String>,
	pub cursor: Option<String>,
	pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
	Text,
	Metadata,
	ModelInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct FileRead {
	pub file_id: Uuid,
	pub thread_id: Option<Uuid>,
	pub area_id: Option<Uuid>,
	pub representation: Representation,
	pub offset: Option<usize>,
	pub max_bytes: Option<usize>,
	pub expected_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct FileEntry {
	pub file_id: Uuid,
	pub path: String,
	pub digest: String,
	pub size: u64,
	pub media_type: String,
	pub scope: FileScope,
	pub provenance: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct Area {
	pub id: Uuid,
	pub tenant: String,
	pub home_node: String,
	pub thread_id: Uuid,
	pub agent_id: String,
	pub owner: String,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub state: String,
	#[serde(skip)]
	pub manifest: Value,
	pub constraints: Value,
}
impl crate::database::native::Decode for Area {
	fn decode(row: &crate::database::native::Row, _: &[&str]) -> crate::Result<Self> {
		Ok(Self {
			id: row.try_get("id")?,
			tenant: row.try_get("tenant")?,
			home_node: row.try_get("home_node")?,
			thread_id: row.try_get("thread_id")?,
			agent_id: row.try_get("agent_id")?,
			owner: row.try_get("owner")?,
			generation: row.try_get("generation")?,
			revision: row.try_get("revision")?,
			epoch: row.try_get("epoch")?,
			state: row.try_get("state")?,
			constraints: row.try_get("constraints")?,
			manifest: serde_json::json!([]),
		})
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct Envelope {
	pub operation_id: String,
	pub status: String,
	pub policy_revision: i64,
	pub area_id: Uuid,
	pub generation: i64,
	pub revision: i64,
	#[serde(flatten)]
	pub result: CapabilityResult,
}

/// Executable result alternatives shared by HTTP and model tool delivery.
/// Result variants allow common envelope fields in composed OpenAPI schemas.
/// Inputs remain strict; outputs permit additive fields for forward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum CapabilityResult {
	Search(SearchResult),
	Read(ReadResult),
	Runtime(RuntimeResult),
	Patch(PatchResult),
	Skills(SkillListResult),
	Skill(SkillReadResult),
	Outbound(OutboundResult),
	/// Scratch Space reads and searches, saves, copies and Background
	/// processes (#745). Last, so it only takes what no other result shape fits.
	Pod(serde_json::Map<String, Value>),
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SearchResult {
	pub matches: Vec<SearchMatch>,
	#[serde(default)]
	pub unavailable: Vec<SearchUnavailable>,
	pub next_cursor: Option<String>,
	pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SearchUnavailable {
	pub file_id: Uuid,
	pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SearchMatch {
	pub file_id: Uuid,
	pub path: String,
	pub digest: String,
	pub location: FileLocation,
	pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct FileLocation {
	pub line: usize,
	pub source: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ReadResult {
	pub metadata: FileEntry,
	pub digest: String,
	pub content: Option<String>,
	pub encoding: Option<String>,
	pub next_offset: Option<usize>,
	pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct RuntimeResult {
	pub kind: String,
	pub epoch: i64,
	pub termination_confirmed: bool,
	pub writer_frozen: bool,
	pub session_id: Option<Uuid>,
	pub displays: Vec<FileEntry>,
	pub exit_code: Option<i64>,
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub output: String,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub output_unchanged: bool,
	pub next_offset: Option<usize>,
	/// When a running command will be stopped. Absent before it starts and after it ends.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub operation_deadline: Option<chrono::DateTime<chrono::Utc>>,
	pub truncated: bool,
	pub effects_may_have_occurred: bool,
	pub error: Option<super::errors::CapabilityError>,
	/// Seconds since the output shown was last fetched from the running command. It keeps growing when the view is stalled, which does not mean the command is. Absent when no output was fetched.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub output_stale_seconds: Option<u64>,
	/// True when the command was still running at its time limit. Absent otherwise.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub deadline_expired: Option<bool>,
	/// Background process adopted when the foreground command exited.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub left_running: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct PatchResult {
	pub changed_paths: Vec<String>,
	pub preimages: Vec<Uuid>,
	pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SkillListResult {
	pub skills: Vec<super::skills::SkillMetadata>,
	pub next_cursor: Option<usize>,
	pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SkillInventoryEntry {
	pub path: String,
	pub digest: String,
	pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SkillReadResult {
	pub skill: super::skills::SkillMetadata,
	pub path: Option<String>,
	pub content: Option<String>,
	pub digest: Option<String>,
	pub encoding: Option<String>,
	pub metadata: Option<FileEntry>,
	pub next_offset: Option<usize>,
	pub truncated: bool,
	pub files: Option<Vec<SkillInventoryEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct OutboundResult {
	pub approval_id: Uuid,
	pub approval_state: String,
	pub approval_revision: i64,
	pub targets: Vec<String>,
	/// Who decided the approval, once someone has; nobody is named while it
	/// waits for an Eligible approver (ADR 0135).
	pub decided_by: Option<String>,
	pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
	pub grant_id: Option<Uuid>,
	pub effects_may_have_occurred: bool,
	pub error: Option<super::errors::CapabilityError>,
	pub output: Option<String>,
	pub output_file: Option<FileEntry>,
	pub truncated: Option<bool>,
	pub http_status: Option<u16>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct AreaPage {
	pub items: Vec<Area>,
	pub next_cursor: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Enqueue {
	pub idempotency_key: Uuid,
	#[validate(length(min = 1, max = 128))]
	pub agent_version: String,
	#[validate(length(min = 1, max = 512))]
	pub title: String,
	#[validate(length(max = 64000))]
	pub description: String,
	/// URL intents: at most five pages the person asks the Run to read. The
	/// host renders them into the description; they never approve a Web
	/// disclosure.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub read_pages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SessionStatus {
	pub area_id: Uuid,
	pub last_run_id: Option<Uuid>,
	pub last_agent_version: Option<String>,
	pub active_run_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Steer {
	pub idempotency_key: Uuid,
	pub expected_run_id: Uuid,
	#[validate(length(min = 1, max = 64000))]
	pub content: String,
	/// URL intents rendered into the content, as for [`Enqueue::read_pages`].
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub read_pages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct Steered {
	pub accepted_run_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Shell {
	pub idempotency_key: Uuid,
	#[validate(length(min = 1, max = 64000))]
	pub command: String,
	/// Seconds the command may run once it starts. It is then stopped, and its changes under /work are saved.
	pub timeout_seconds: Option<u64>,
	/// How long the Run waits for completion; zero returns immediately.
	pub wait_seconds: Option<u64>,
	#[validate(range(min = 0))]
	/// The working area revision: take it from the previous result's `revision`; a new area starts at 1.
	pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct LivePreviewStart {
	/// The command whose Web server listens on `port`.
	#[validate(length(min = 1, max = 64000))]
	pub command: String,
	/// The port the Pod declares; only the Deployment's preview Pod port is admitted.
	pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct LivePreviewStop {}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Patch {
	pub idempotency_key: Uuid,
	pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct OperationInput {
	pub operation_id: Uuid,
	pub offset: Option<usize>,
	/// How long the Run waits for completion; zero returns immediately.
	pub wait_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct OperationResult {
	pub operation_id: Uuid,
	pub kind: String,
	pub status: String,
	pub area_id: Uuid,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub policy_revision: i64,
	pub termination_confirmed: bool,
	pub writer_frozen: bool,
	pub session_id: Option<Uuid>,
	pub displays: Vec<FileEntry>,
	pub exit_code: Option<i64>,
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub output: String,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub output_unchanged: bool,
	pub next_offset: Option<usize>,
	/// When a running command will be stopped. Absent before it starts and after it ends.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub operation_deadline: Option<chrono::DateTime<chrono::Utc>>,
	pub truncated: bool,
	pub effects_may_have_occurred: bool,
	pub error: Option<super::errors::CapabilityError>,
	/// Seconds since the output shown was last fetched from the running command. It keeps growing when the view is stalled, which does not mean the command is. Absent when no output was fetched.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub output_stale_seconds: Option<u64>,
	/// True when the command was still running at its time limit. Absent otherwise.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub deadline_expired: Option<bool>,
	/// Background process adopted when the foreground command exited.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub left_running: Option<String>,
}

use uuid::Uuid;

/// Read a Reference file by `file_id` or a Scratch Space file by `path`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct ToolFileRead {
	/// A Reference file. Leave it out to read `path`.
	pub file_id: Option<Uuid>,
	/// A file in the Scratch Space, relative to its root. Leave out `file_id`.
	pub path: Option<String>,
	pub representation: Option<Representation>,
	pub offset: Option<usize>,
	pub max_bytes: Option<usize>,
	pub expected_digest: Option<String>,
}

/// Where `file_search` looks; `scratch` is the Run's Scratch Space (#745).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolFileScope {
	References,
	Scratch,
}

/// `file_search` as Agents call it (#745).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct ToolFileSearch {
	/// May be empty in `path` mode, which then lists every path in the scope.
	#[validate(length(max = 1024))]
	pub query: String,
	pub mode: SearchMode,
	pub scope: ToolFileScope,
	pub path: Option<String>,
	/// Scratch Space only: a glob such as `src/*.rs` the whole path must match.
	pub glob: Option<String>,
	pub cursor: Option<String>,
	pub limit: Option<usize>,
}

/// Register Scratch Space files in the Working Area (#745).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileSave {
	pub idempotency_key: Uuid,
	/// Files or directories relative to the Scratch Space root; `.` is all of it.
	pub paths: Vec<String>,
	/// Files whose gitignore rules apply in each directory. Defaults to `.gitignore`.
	pub ignore_files: Option<Vec<String>>,
	/// Patterns to leave out, beating `ignore_files`.
	pub exclude: Option<Vec<String>>,
	/// Patterns to save even when excluded or ignored.
	pub include: Option<Vec<String>>,
	/// Also remove registered files under `paths` that the selection no longer holds.
	pub sync: Option<bool>,
	/// The working area revision from the previous result, to refuse a save over a newer one.
	pub expected_revision: Option<i64>,
}

/// Remove registered files, or everything under a directory, from the Working Area.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRemove {
	pub idempotency_key: Uuid,
	pub paths: Vec<String>,
	/// The working area revision from the previous result.
	pub expected_revision: Option<i64>,
}

/// Copy registered Working Area files into the Scratch Space (#745).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileCopy {
	pub idempotency_key: Uuid,
	/// Registered files or directories.
	pub paths: Vec<String>,
	/// A Scratch Space directory to copy under; by default each file keeps its path.
	pub destination: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessList {}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStream {
	Stdout,
	Stderr,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessLog {
	pub operation_id: String,
	pub stream: Option<ProcessStream>,
	/// Byte offset to read from; without it the last `limit` bytes are returned.
	pub offset: Option<u64>,
	/// At most 262144 bytes; 65536 by default.
	pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessStop {
	pub operation_id: String,
}
