//! Admission preserves draft authority and tenant-concurrency locking until the session is committed.
use super::{
	SandboxRepository,
	execution::{ExecutionRepository, ExecutionScope},
};
use crate::{Result, ports::ModelProvider};
use aidash_domain::{
	model::ModelConfig,
	registry::workbench::{
		Draft,
		sandbox::{TestLimits, TestSession},
	},
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
#[async_trait]
pub trait AdmissionScope: ExecutionScope {
	/// The tenant-limits lock is held across expiry, count, active-slot insertion and commit.
	async fn admit(
		&mut self,
		draft: &Draft,
		limits: &TestLimits,
		scenario: Value,
		conversation: Value,
	) -> Result<TestSession>;
}
#[async_trait]
pub trait AdmissionRepository: ExecutionRepository + SandboxRepository {
	async fn begin_admission(&self) -> Result<Box<dyn AdmissionScope + '_>>;
}
pub trait SandboxModels: Send + Sync {
	fn provider(&self, model: ModelConfig) -> Result<Arc<dyn ModelProvider>>;
}
