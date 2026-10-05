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
	#[validate(length(min = 1, max = 1024))]
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
	pub workspace_id: Uuid,
	pub thread_id: Uuid,
	pub agent_id: String,
	pub owner: String,
	pub generation: i64,
	pub revision: i64,
	pub epoch: i64,
	pub state: String,
	pub manifest: Value,
	pub constraints: Value,
	pub next_sequence: i64,
}
crate::native_record!(Area {
	id,
	tenant,
	home_node,
	workspace_id,
	thread_id,
	agent_id,
	owner,
	generation,
	revision,
	epoch,
	state,
	manifest,
	constraints,
	next_sequence
});

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
	Reset(PythonReset),
	Patch(PatchResult),
	Skills(SkillListResult),
	Skill(SkillReadResult),
	Share(ShareResult),
	Outbound(OutboundResult),
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
	pub output: String,
	pub next_offset: Option<usize>,
	pub truncated: bool,
	pub effects_may_have_occurred: bool,
	pub error: Option<super::errors::CapabilityError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct PythonReset {
	pub session_id: Uuid,
	pub session_reset: bool,
	pub reset_reason: String,
	pub packages: PythonEnvironment,
	pub error: super::errors::CapabilityError,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct PythonEnvironment {
	pub image: Option<String>,
	pub overlay_manifest: Option<FileEntry>,
	pub dependencies: Option<PackageManifest>,
	pub automatic_reinstall: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct PackageManifest {
	pub protocol: String,
	pub image: String,
	pub packages: Vec<InstalledPackage>,
	pub outcome: String,
	pub network: String,
	pub automatic_reinstall: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct InstalledPackage {
	pub path: String,
	pub filename: String,
	pub name: String,
	pub version: String,
	pub sha256: String,
	pub outbound_operation_id: Uuid,
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
pub struct FileReceipt {
	pub id: Uuid,
	pub node_id: Option<String>,
	pub area_id: Uuid,
	pub revision: i64,
	pub manifest_digest: Option<String>,
	pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ShareResult {
	pub snapshot_id: Option<Uuid>,
	pub transfer_id: Uuid,
	pub manifest_digest: String,
	pub recipient: super::sharing::Recipient,
	pub receipt: Option<FileReceipt>,
	pub error: Option<super::errors::CapabilityError>,
	pub effects_may_have_occurred: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct OutboundResult {
	pub approval_id: Uuid,
	pub approval_state: String,
	pub approval_revision: i64,
	pub targets: Vec<String>,
	pub approver: Option<String>,
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
pub struct Materialize {
	pub idempotency_key: Uuid,
	#[validate(range(min = 0))]
	pub expected_revision: i64,
	pub source: MaterializeSource,
	#[validate(length(min = 1, max = 4096))]
	pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SessionRun {
	pub run_id: Uuid,
	pub sequence: i64,
	pub phase: String,
	pub control: String,
}
crate::native_record!(SessionRun {
	run_id,
	sequence,
	phase,
	control
});

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct SessionStatus {
	pub area_id: Uuid,
	pub last_run_id: Option<Uuid>,
	pub last_agent_version: Option<String>,
	pub active_run_id: Option<Uuid>,
	pub queue: Vec<SessionRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Steer {
	pub idempotency_key: Uuid,
	pub expected_run_id: Uuid,
	#[validate(length(min = 1, max = 64000))]
	pub content: String,
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
	pub timeout_seconds: Option<u64>,
	#[validate(range(min = 0))]
	pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Patch {
	pub idempotency_key: Uuid,
	#[validate(range(min = 0))]
	pub expected_revision: i64,
	/// Every changed path must have a SHA-256 digest, or null to require absence.
	pub preconditions: std::collections::BTreeMap<String, Option<String>>,
	pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct OperationInput {
	pub operation_id: Uuid,
	pub offset: Option<usize>,
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
	pub output: String,
	pub next_offset: Option<usize>,
	pub truncated: bool,
	pub effects_may_have_occurred: bool,
	pub error: Option<super::errors::CapabilityError>,
}

use uuid::Uuid;
