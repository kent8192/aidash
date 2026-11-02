//! Runtime boundary for the long-lived Execution Pod belonging to a Scratch Space.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ExecutionPodError {
	#[error("{code}: {message}")]
	Remote { code: String, message: String },
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
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodReceipt {
	pub protocol: String,
	pub pod: String,
	pub claim: String,
	#[serde(default)]
	pub reset: bool,
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
	fn interrupted_command_preserves_unknown_outcome_and_binary_output() {
		let result: CommandResult = serde_json::from_str(r#"{"operation_id":"op","status":"interrupted","outcome":"unknown","stdout":"AP8=","error_code":"command_interrupted","message":"node died"}"#).unwrap();
		assert_eq!(result.outcome.as_deref(), Some("unknown"));
		assert_eq!(result.stdout.as_deref(), Some("AP8="));
		assert_eq!(result.error_code.as_deref(), Some("command_interrupted"));
	}
}
