//! Runtime boundary for the long-lived Execution Pod belonging to a Scratch Space.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionPodErrorCode {
	TerminationUnconfirmed,
	PodUnavailable,
	CommandConflict,
	InvalidRequest,
	Timeout,
	Interrupted,
	QueueFull,
	AdmissionFailed,
	AdmissionUnverified,
	CapacityMismatch,
	ChangeLogUnavailable,
	CommandNotFound,
	NotBackground,
	PodIdentityMissing,
	PodInterrupted,
	PurgeUnconfirmed,
	PurgeUnsupported,
	SettingsMismatch,
	StartupFailed,
	StartupTimeout,
	StopFailed,
	CommandInterrupted,
	RequestRejected,
	RunnerUnavailable,
	PythonUnavailable,
	SessionReset,
	InvalidPath,
	PreviewStartFailed,
	WatcherStartFailed,
	WheelDiskFull,
	RunnerDiskFull,
	PodProcessLimit,
	/// The operation id was refused before the Runner accepted it.
	CommandRefused,
	Unknown(String),
}
impl From<&str> for ExecutionPodErrorCode {
	fn from(value: &str) -> Self {
		match value {
			"termination_unconfirmed" => Self::TerminationUnconfirmed,
			"pod_unavailable" => Self::PodUnavailable,
			"command_conflict" => Self::CommandConflict,
			"invalid_request" => Self::InvalidRequest,
			"timeout" => Self::Timeout,
			"interrupted" => Self::Interrupted,
			"queue_full" => Self::QueueFull,
			"admission_failed" => Self::AdmissionFailed,
			"admission_unverified" => Self::AdmissionUnverified,
			"capacity_mismatch" => Self::CapacityMismatch,
			"change_log_unavailable" => Self::ChangeLogUnavailable,
			"command_not_found" => Self::CommandNotFound,
			"not_background" => Self::NotBackground,
			"pod_identity_missing" => Self::PodIdentityMissing,
			"pod_interrupted" => Self::PodInterrupted,
			"purge_unconfirmed" => Self::PurgeUnconfirmed,
			"purge_unsupported" => Self::PurgeUnsupported,
			"settings_mismatch" => Self::SettingsMismatch,
			"startup_failed" => Self::StartupFailed,
			"startup_timeout" => Self::StartupTimeout,
			"stop_failed" => Self::StopFailed,
			"command_interrupted" => Self::CommandInterrupted,
			"request_rejected" => Self::RequestRejected,
			"runner_unavailable" => Self::RunnerUnavailable,
			"python_unavailable" => Self::PythonUnavailable,
			"session_reset" => Self::SessionReset,
			"invalid_path" => Self::InvalidPath,
			"preview_start_failed" => Self::PreviewStartFailed,
			"pod_process_limit" => Self::PodProcessLimit,
			"runner_disk_full" => Self::RunnerDiskFull,
			"wheel_disk_full" => Self::WheelDiskFull,
			"watcher_start_failed" => Self::WatcherStartFailed,
			"command_refused" => Self::CommandRefused,
			other => Self::Unknown(other.into()),
		}
	}
}
impl std::fmt::Display for ExecutionPodErrorCode {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let value = match self {
			Self::TerminationUnconfirmed => "termination_unconfirmed",
			Self::PodUnavailable => "pod_unavailable",
			Self::CommandConflict => "command_conflict",
			Self::InvalidRequest => "invalid_request",
			Self::Timeout => "timeout",
			Self::Interrupted => "interrupted",
			Self::QueueFull => "queue_full",
			Self::AdmissionFailed => "admission_failed",
			Self::AdmissionUnverified => "admission_unverified",
			Self::CapacityMismatch => "capacity_mismatch",
			Self::ChangeLogUnavailable => "change_log_unavailable",
			Self::CommandNotFound => "command_not_found",
			Self::NotBackground => "not_background",
			Self::PodIdentityMissing => "pod_identity_missing",
			Self::PodInterrupted => "pod_interrupted",
			Self::PurgeUnconfirmed => "purge_unconfirmed",
			Self::PurgeUnsupported => "purge_unsupported",
			Self::SettingsMismatch => "settings_mismatch",
			Self::StartupFailed => "startup_failed",
			Self::StartupTimeout => "startup_timeout",
			Self::StopFailed => "stop_failed",
			Self::CommandInterrupted => "command_interrupted",
			Self::RequestRejected => "request_rejected",
			Self::RunnerUnavailable => "runner_unavailable",
			Self::PythonUnavailable => "python_unavailable",
			Self::SessionReset => "session_reset",
			Self::InvalidPath => "invalid_path",
			Self::PreviewStartFailed => "preview_start_failed",
			Self::PodProcessLimit => "pod_process_limit",
			Self::RunnerDiskFull => "runner_disk_full",
			Self::WheelDiskFull => "wheel_disk_full",
			Self::WatcherStartFailed => "watcher_start_failed",
			Self::CommandRefused => "command_refused",
			Self::Unknown(value) => value,
		};
		f.write_str(value)
	}
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionPodError {
	#[error("{code}: {message}")]
	Remote {
		code: ExecutionPodErrorCode,
		message: String,
	},
	#[error("execution pod transport: {0}")]
	Transport(#[from] crate::Error),
}
/// Local transport deadline. The Runner may already have accepted the request.
#[derive(Debug, thiserror::Error)]
#[error("Runner route {route} did not answer within {seconds} seconds")]
pub struct RunnerTimeout {
	pub route: String,
	pub seconds: u64,
}

impl RunnerTimeout {
	pub const CODE: &str = "runner_timeout";
}

pub type Result<T> = std::result::Result<T, ExecutionPodError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPodSettings {
	pub scratch_space_id: String,
	pub image: String,
	pub cpu: u32,
	pub memory_bytes: u64,
	pub capacity_bytes: u64,
	pub docker_enabled: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tenant_id: Option<String>,
}
/// Serializes to the Runner as is, and never appears in `Debug` output.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct EgressCredential(pub String);
impl std::fmt::Debug for EgressCredential {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("EgressCredential(..)")
	}
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodReceipt {
	pub protocol: String,
	pub pod: String,
	pub claim: String,
	#[serde(default)]
	pub reset: bool,
	/// The persisted system tree was initialized from a different or unknown image.
	#[serde(default)]
	pub image_changed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRequest {
	pub operation_id: String,
	pub scratch_space_id: String,
	pub run_id: String,
	pub code: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub expected_session_id: Option<uuid::Uuid>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub cwd: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub timeout_seconds: Option<u32>,
	#[serde(default)]
	pub background: bool,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub notify_on_exit: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub proxy_credential: Option<EgressCredential>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommandResult {
	pub operation_id: String,
	pub status: String,
	/// UID from the Runner journal at command admission.
	pub pod_uid: Option<String>,
	pub scratch_space_id: Option<String>,
	pub run_id: Option<String>,
	pub background: Option<bool>,
	pub outcome: Option<String>,
	pub exit_code: Option<i32>,
	/// Base64-encoded bytes, bounded by the Runner's output limit.
	pub stdout: Option<String>,
	pub stderr: Option<String>,
	pub truncated: Option<bool>,
	pub error_code: Option<String>,
	pub message: Option<String>,
	pub python_result: Option<Value>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub session_id: Option<uuid::Uuid>,
	pub left_running: Option<String>,
	pub stop_reason: Option<String>,
	/// Base64-encoded tail of the output, from both streams.
	pub output_tail: Option<String>,
	/// The Run's Python interpreter was started afresh for this cell (the Pod
	/// was replaced), so earlier variables are gone.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_replaced: Option<bool>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStopReason {
	RunCancelled,
	RunAuthorityRevoked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BackgroundProcesses {
	pub processes: Vec<CommandResult>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PythonSession {
	pub operation_id: String,
	pub scratch_space_id: String,
	pub run_id: String,
	pub socket_path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangeLogChunk {
	pub data: String,
	pub next_offset: u64,
	pub eof: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PythonStartRequest {
	pub scratch_space_id: String,
	pub run_id: String,
	pub proxy_credential: Option<EgressCredential>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogRequest {
	pub operation_id: String,
	pub stream: String,
	pub offset: Option<u64>,
	pub limit: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogChunk {
	pub data: String,
	pub next_offset: u64,
	pub eof: bool,
	pub size: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileReadRequest {
	pub scratch_space_id: String,
	pub path: String,
	pub offset: Option<u64>,
	pub limit: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileChunk {
	pub data: String,
	pub size: u64,
	pub next_offset: u64,
	pub eof: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSearchRequest {
	pub scratch_space_id: String,
	pub pattern: String,
	pub path: Option<String>,
	pub glob: Option<String>,
	pub regex: bool,
	pub max_results: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileMatch {
	pub path: String,
	pub line: u64,
	pub text: String,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileSearchResult {
	pub matches: Vec<FileMatch>,
	pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilePatchRequest {
	pub scratch_space_id: String,
	pub run_id: String,
	pub claim_id: uuid::Uuid,
	/// Only the original admission can apply the patch. Retries reconcile its receipt.
	pub apply: bool,
	pub patch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilePatchResult {
	pub changed: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileWriteRequest {
	pub scratch_space_id: String,
	pub run_id: String,
	pub path: String,
	pub data: String,
	pub offset: u64,
	#[serde(rename = "final")]
	pub final_chunk: bool,
	pub mode: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileWriteResult {
	pub size: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileManifestRequest {
	pub scratch_space_id: String,
	pub root: String,
	pub ignore_files: Vec<String>,
	pub exclude: Vec<String>,
	pub include: Vec<String>,
	/// Search asks for a bounded listing; saves need the complete inventory.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_files: Option<u32>,
	/// A `root` that does not exist lists no files instead of failing, so a
	/// sync save can select a deleted path.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub missing_ok: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
	pub path: String,
	pub size: u64,
	pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileManifestResult {
	pub files: Vec<ManifestFile>,
	pub skipped_symlinks: Vec<String>,
	pub truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewStartRequest {
	pub operation_id: String,
	pub scratch_space_id: String,
	pub run_id: String,
	pub code: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub cwd: Option<String>,
	pub port: u16,
	pub proxy_credential: Option<EgressCredential>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewTarget {
	pub operation_id: String,
	pub address: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpStartRequest {
	pub operation_id: String,
	pub scratch_space_id: String,
	pub run_id: String,
	pub install: String,
	pub command: Vec<String>,
	pub env: std::collections::BTreeMap<String, String>,
	pub plugin_data: String,
	pub proxy_credential: Option<EgressCredential>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpSession {
	pub operation_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpCallRequest {
	pub operation_id: String,
	pub message: Value,
	pub timeout_seconds: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpCallResult {
	pub message: Value,
}

/// The Runner's answer to refusing an operation id it may not have received.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandRefusal {
	/// True when the id was never accepted: a later submit of it is refused.
	pub refused: bool,
	/// The accepted command's status when it was not refused.
	#[serde(default)]
	pub status: Option<String>,
}

/// Calls are idempotent only where the Runner's operation ID contract says so;
/// an interrupted operation has an unknown outcome and must never be replayed.
#[async_trait]
pub trait ExecutionPodRuntime: Send + Sync {
	async fn ensure(&self, settings: &ExecutionPodSettings) -> Result<PodReceipt>;
	async fn execute(&self, request: &CommandRequest) -> Result<CommandResult>;
	async fn result(&self, operation_id: &str) -> Result<CommandResult>;
	/// Refuse an operation id before the Runner accepts it, so a delayed submit
	/// of it can never start (#745). Idempotent.
	async fn refuse_command(
		&self,
		_operation_id: &str,
		_scratch_space_id: &str,
		_run_id: &str,
	) -> Result<CommandRefusal> {
		Err(ExecutionPodError::Transport(crate::Error::External(
			"this runtime cannot refuse a command".into(),
		)))
	}
	async fn cancel_waiting(&self, scratch_space_id: &str, run_id: &str) -> Result<()>;
	/// Stop one foreground command; the Run's other commands keep running.
	async fn stop_command(&self, _operation_id: &str) -> Result<CommandResult> {
		Err(ExecutionPodError::Transport(crate::Error::External(
			"this runtime cannot stop one command".into(),
		)))
	}
	async fn backgrounds(&self, scratch_space_id: &str) -> Result<BackgroundProcesses>;
	async fn stop_background(&self, operation_id: &str) -> Result<CommandResult>;
	async fn python_start(&self, request: &PythonStartRequest) -> Result<PythonSession>;
	async fn stop_run(
		&self,
		scratch_space_id: &str,
		run_id: &str,
		reason: RunStopReason,
	) -> Result<Vec<String>>;
	async fn background_log(&self, request: &LogRequest) -> Result<LogChunk>;
	async fn file_read(&self, request: &FileReadRequest) -> Result<FileChunk>;
	async fn file_search(&self, request: &FileSearchRequest) -> Result<FileSearchResult>;
	async fn file_patch(&self, request: &FilePatchRequest) -> Result<FilePatchResult>;
	async fn file_write(&self, request: &FileWriteRequest) -> Result<FileWriteResult>;
	async fn file_manifest(&self, request: &FileManifestRequest) -> Result<FileManifestResult>;
	async fn preview_start(&self, request: &PreviewStartRequest) -> Result<PreviewTarget>;
	async fn preview_stop(&self, operation_id: &str) -> Result<()>;
	async fn mcp_start(&self, request: &McpStartRequest) -> Result<McpSession>;
	async fn mcp_call(&self, request: &McpCallRequest) -> Result<McpCallResult>;
	async fn mcp_stop(&self, operation_id: &str) -> Result<()>;
	/// Stop every stdio MCP session the Runner holds for the Run in this Scratch Space,
	/// including one whose start the caller lost track of. Other Background processes of
	/// the Run keep running. Returns once each session ended.
	async fn mcp_stop_run(&self, _scratch_space_id: &str, _run_id: &str) -> Result<()> {
		Err(ExecutionPodError::Transport(crate::Error::External(
			"this runtime cannot stop a Run's MCP sessions".into(),
		)))
	}
	async fn python_cell(&self, request: &CommandRequest) -> Result<CommandResult>;
	async fn python_stop(&self, scratch_space_id: &str, run_id: &str) -> Result<()>;
	async fn change_log(
		&self,
		scratch_space_id: &str,
		offset: u64,
		limit: u32,
	) -> Result<ChangeLogChunk>;
	async fn viewer_heartbeat(&self, scratch_space_id: &str, viewer_id: &str) -> Result<()>;
	async fn reset(&self, settings: &ExecutionPodSettings) -> Result<PodReceipt>;
	async fn stop(&self, scratch_space_id: &str) -> Result<()>;
	async fn delete(&self, scratch_space_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn pod_receipt_preserves_image_change_and_defaults_for_older_runners() {
		for (wire, expected) in [
			(
				r#"{"protocol":"aidash-runner/2","pod":"p","claim":"c","image_changed":true}"#,
				true,
			),
			(
				r#"{"protocol":"aidash-runner/2","pod":"p","claim":"c"}"#,
				false,
			),
		] {
			let receipt: PodReceipt = serde_json::from_str(wire).unwrap();
			assert_eq!(receipt.image_changed, expected);
		}
	}
	#[test]
	fn remote_error_codes_preserve_known_and_future_values() {
		assert_eq!(
			ExecutionPodErrorCode::from("termination_unconfirmed"),
			ExecutionPodErrorCode::TerminationUnconfirmed
		);
		for (wire, expected) in [
			(
				"python_unavailable",
				ExecutionPodErrorCode::PythonUnavailable,
			),
			("invalid_path", ExecutionPodErrorCode::InvalidPath),
			(
				"preview_start_failed",
				ExecutionPodErrorCode::PreviewStartFailed,
			),
			("pod_process_limit", ExecutionPodErrorCode::PodProcessLimit),
			("runner_disk_full", ExecutionPodErrorCode::RunnerDiskFull),
			("wheel_disk_full", ExecutionPodErrorCode::WheelDiskFull),
			(
				"watcher_start_failed",
				ExecutionPodErrorCode::WatcherStartFailed,
			),
			("command_refused", ExecutionPodErrorCode::CommandRefused),
		] {
			let code = ExecutionPodErrorCode::from(wire);
			assert_eq!(code, expected);
			assert_eq!(code.to_string(), wire);
		}
		assert_eq!(
			ExecutionPodErrorCode::from("future_code"),
			ExecutionPodErrorCode::Unknown("future_code".into())
		);
	}
	#[test]
	fn command_result_carries_adopted_background_operation() {
		let result: CommandResult = serde_json::from_value(serde_json::json!({
			"operation_id":"op", "status":"completed", "left_running":"op.left"
		}))
		.unwrap();
		assert_eq!(result.left_running.as_deref(), Some("op.left"));
		assert_eq!(
			serde_json::to_value(result).unwrap()["left_running"],
			"op.left"
		);
	}
	#[test]
	fn interrupted_command_preserves_unknown_outcome_and_binary_output() {
		let result: CommandResult = serde_json::from_str(r#"{"operation_id":"op","status":"interrupted","outcome":"unknown","stdout":"AP8=","error_code":"command_interrupted","message":"node died"}"#).unwrap();
		assert_eq!(result.outcome.as_deref(), Some("unknown"));
		assert_eq!(result.stdout.as_deref(), Some("AP8="));
		assert_eq!(result.error_code.as_deref(), Some("command_interrupted"));
	}
}

#[cfg(test)]
mod egress_credential_tests {
	use super::EgressCredential;

	#[test]
	fn it_reaches_the_runner_but_not_a_debug_line() {
		let credential = EgressCredential("canary".into());
		assert_eq!(serde_json::to_string(&credential).unwrap(), r#""canary""#);
		assert!(!format!("{credential:?}").contains("canary"));
	}

	#[test]
	fn process_credential_uses_the_wire_name_and_is_redacted() {
		let request = super::PythonStartRequest {
			scratch_space_id: "s".into(),
			run_id: "r".into(),
			proxy_credential: Some(EgressCredential("canary".into())),
		};
		let value = serde_json::to_value(&request).unwrap();
		assert_eq!(value["proxy_credential"], "canary");
		assert!(!format!("{request:?}").contains("canary"));
	}
}
