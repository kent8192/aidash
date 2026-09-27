use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileScope {
	Working,
	References,
	Received,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
	Literal,
	Regex,
	Path,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileSearch {
	#[schema(min_length = 1, max_length = 1024)]
	pub query: String,
	pub mode: SearchMode,
	pub scope: FileScope,
	pub path: Option<String>,
	pub cursor: Option<String>,
	#[schema(minimum = 1, maximum = 50)]
	pub limit: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Representation {
	Text,
	Metadata,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FileRead {
	pub file_id: Uuid,
	pub representation: Representation,
	pub offset: Option<usize>,
	#[schema(minimum = 1, maximum = 16384)]
	pub max_bytes: Option<usize>,
	pub expected_digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FileEntry {
	pub file_id: Uuid,
	pub path: String,
	pub digest: String,
	pub size: u64,
	pub media_type: String,
	pub scope: FileScope,
	pub provenance: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchResult {
	#[schema(max_items = 50)]
	pub matches: Vec<SearchMatch>,
	#[serde(default)]
	pub unavailable: Vec<SearchUnavailable>,
	pub next_cursor: Option<String>,
	pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchUnavailable {
	pub file_id: Uuid,
	pub error: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchMatch {
	pub file_id: Uuid,
	pub path: String,
	pub digest: String,
	pub location: FileLocation,
	pub snippet: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FileLocation {
	pub line: usize,
	pub source: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReadResult {
	pub metadata: FileEntry,
	pub digest: String,
	pub content: Option<String>,
	pub encoding: Option<String>,
	pub next_offset: Option<usize>,
	pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PythonReset {
	pub session_id: Uuid,
	pub session_reset: bool,
	pub reset_reason: String,
	pub packages: PythonEnvironment,
	pub error: super::errors::CapabilityError,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PythonEnvironment {
	pub image: Option<String>,
	pub overlay_manifest: Option<FileEntry>,
	pub dependencies: Option<PackageManifest>,
	pub automatic_reinstall: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PackageManifest {
	pub protocol: String,
	pub image: String,
	pub packages: Vec<InstalledPackage>,
	pub outcome: String,
	pub network: String,
	pub automatic_reinstall: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct InstalledPackage {
	pub path: String,
	pub filename: String,
	pub name: String,
	pub version: String,
	pub sha256: String,
	pub outbound_operation_id: Uuid,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PatchResult {
	pub changed_paths: Vec<String>,
	pub preimages: Vec<Uuid>,
	pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillListResult {
	pub skills: Vec<super::skills::SkillMetadata>,
	pub next_cursor: Option<usize>,
	pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SkillInventoryEntry {
	pub path: String,
	pub digest: String,
	pub size: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FileReceipt {
	pub id: Uuid,
	pub node_id: Option<String>,
	pub area_id: Uuid,
	pub revision: i64,
	pub manifest_digest: Option<String>,
	pub files: Vec<FileEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ShareResult {
	pub snapshot_id: Option<Uuid>,
	pub transfer_id: Uuid,
	pub manifest_digest: String,
	pub recipient: super::sharing::Recipient,
	pub receipt: Option<FileReceipt>,
	pub error: Option<super::errors::CapabilityError>,
	pub effects_may_have_occurred: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct AreaPage {
	pub items: Vec<Area>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Materialize {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	pub source: MaterializeSource,
	pub path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Enqueue {
	pub idempotency_key: Uuid,
	pub agent_version: String,
	pub title: String,
	pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SessionRun {
	pub run_id: Uuid,
	pub sequence: i64,
	pub phase: String,
	pub control: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionStatus {
	pub area_id: Uuid,
	pub last_run_id: Option<Uuid>,
	pub last_agent_version: Option<String>,
	pub active_run_id: Option<Uuid>,
	pub queue: Vec<SessionRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Steer {
	pub idempotency_key: Uuid,
	pub expected_run_id: Uuid,
	pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Steered {
	pub accepted_run_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Shell {
	pub idempotency_key: Uuid,
	#[schema(min_length = 1, max_length = 65536)]
	pub command: String,
	#[schema(minimum = 1, maximum = 600, default = 120)]
	pub timeout_seconds: Option<u64>,
	pub expected_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	/// Every changed path must have a SHA-256 digest, or null to require absence.
	pub preconditions: std::collections::BTreeMap<String, Option<String>>,
	#[schema(min_length = 1, max_length = 262144)]
	pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationInput {
	pub operation_id: Uuid,
	pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
