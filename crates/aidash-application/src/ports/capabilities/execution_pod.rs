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
	/// The Pod's proof to the Egress gateway that it belongs to this Scratch Space.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub egress_credential: Option<EgressCredential>,
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
	pub cwd: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub timeout_seconds: Option<u32>,
	#[serde(default)]
	pub background: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommandResult {
	pub operation_id: String,
	pub status: String,
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

/// Calls are idempotent only where the Runner's operation ID contract says so;
/// an interrupted operation has an unknown outcome and must never be replayed.
#[async_trait]
pub trait ExecutionPodRuntime: Send + Sync {
	async fn ensure(&self, settings: &ExecutionPodSettings) -> Result<PodReceipt>;
	async fn execute(&self, request: &CommandRequest) -> Result<CommandResult>;
	async fn result(&self, operation_id: &str) -> Result<CommandResult>;
	async fn cancel_waiting(&self, scratch_space_id: &str, run_id: &str) -> Result<()>;
	async fn backgrounds(&self, scratch_space_id: &str) -> Result<BackgroundProcesses>;
	async fn stop_background(&self, operation_id: &str) -> Result<CommandResult>;
	async fn python_start(&self, scratch_space_id: &str, run_id: &str) -> Result<PythonSession>;
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
		assert_eq!(
			ExecutionPodErrorCode::from("future_code"),
			ExecutionPodErrorCode::Unknown("future_code".into())
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
	fn the_runner_finds_it_under_the_name_it_reads() {
		let settings = super::ExecutionPodSettings {
			scratch_space_id: "s".into(),
			image: "image".into(),
			cpu: 1,
			memory_bytes: 1,
			capacity_bytes: 1,
			docker_enabled: false,
			tenant_id: None,
			egress_credential: Some(EgressCredential("canary".into())),
		};
		let value = serde_json::to_value(&settings).unwrap();
		assert_eq!(value["egress_credential"], "canary");
		assert!(!format!("{settings:?}").contains("canary"));
	}
}
