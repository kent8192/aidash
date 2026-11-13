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
	pub workspace_id: Option<Uuid>,
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
impl crate::database::native::Decode for Area {
	fn decode(row: &crate::database::native::Row, _: &[&str]) -> crate::Result<Self> {
		Ok(Self {
			id: row.try_get("id")?,
			tenant: row.try_get("tenant")?,
			home_node: row.try_get("home_node")?,
			workspace_id: row.try_get("workspace_id")?,
			thread_id: row.try_get("thread_id")?,
			agent_id: row.try_get("agent_id")?,
			owner: row.try_get("owner")?,
			generation: row.try_get("generation")?,
			revision: row.try_get("revision")?,
			epoch: row.try_get("epoch")?,
			state: row.try_get("state")?,
			constraints: row.try_get("constraints")?,
			next_sequence: row.try_get("next_sequence")?,
			manifest: row
				.try_get::<Option<Value>>("manifest")?
				.ok_or_else(|| crate::Error::Invalid("missing area manifest revision".into()))?,
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
pub struct Materialize {
	pub idempotency_key: Uuid,
	#[validate(range(min = 0))]
	/// The working area revision: take it from the previous result's `revision`; a new area starts at 1.
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
	/// URL intents: at most five pages the person asks the Run to read. The
	/// host renders them into the description; they never approve a Web
	/// disclosure.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub read_pages: Vec<String>,
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
	#[validate(range(min = 0))]
	/// The working area revision: take it from the previous result's `revision`; a new area starts at 1.
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
}

use uuid::Uuid;
