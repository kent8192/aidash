//! What Agent tools that run in the Execution Pod of a Scratch Space need from
//! their adapters (#745, ADR 0136).
use super::execution_pod::{EgressCredential, ExecutionPodRuntime, ExecutionPodSettings};
use crate::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

/// The Execution Pod a Run's tools reach, as its Scratch Space is now.
#[derive(Clone)]
pub struct PodContext {
	pub runtime: Arc<dyn ExecutionPodRuntime>,
	/// Recomputed from the Scratch Space's current settings, never stored.
	pub settings: ExecutionPodSettings,
	/// The Run credential of ADR 0136. Recomputed for each request and never
	/// persisted, so a retried request carries the same value.
	pub credential: Option<EgressCredential>,
}
impl PodContext {
	pub fn scratch_space_id(&self) -> &str {
		&self.settings.scratch_space_id
	}
}

/// A durable command or Python cell that runs in an Execution Pod.
#[derive(Clone, Debug, PartialEq)]
pub struct PodOperation {
	pub id: Uuid,
	pub tenant: String,
	pub run_id: Uuid,
	/// `shell` or `code_interpreter`.
	pub kind: String,
	pub state: String,
	/// The admitted request, including `scratch_space_id`, `command` and
	/// `seconds`. It never holds the Run credential.
	pub input: Value,
	pub result: Value,
	/// The Run has finished, so its running work is stopped.
	pub run_finished: bool,
}

/// The capability worker's view of Pod operations. Every call commits on its own.
#[async_trait]
pub trait PodOperationRepository: Send + Sync {
	/// Active Pod operations, oldest update first, at most a small batch.
	async fn pending(&self) -> Result<Vec<Uuid>>;
	async fn load(&self, id: Uuid) -> Result<PodOperation>;
	/// The Pod this operation runs in, with its Run's credential.
	async fn context(&self, operation: &PodOperation) -> Result<PodContext>;
	/// Record a new state and result, only while the row is still in `expected`.
	/// Returns whether it was written.
	async fn save(&self, id: Uuid, expected: &str, state: &str, result: &Value) -> Result<bool>;
}
