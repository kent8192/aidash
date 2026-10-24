//! Run controls borrow one transaction through grant locking, mutation, audit and commit.
use crate::Result;
use aidash_domain::{
	RawRun, RunControlAction, RunInspection,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
pub trait RunControlScope: Send {
	fn identity(&self) -> ExecutionPrincipal;
	fn set_context(&mut self, context: Value);
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	async fn run(&mut self, id: Uuid) -> Result<Option<RawRun>>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn run_visible(&mut self, run: &RunInspection) -> Result<bool>;
	/// The resume grant remains locked FOR UPDATE until the surrounding transaction finishes.
	async fn lock_execution_grant(&mut self, id: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn update_credential(&mut self, id: Uuid, credential: Uuid) -> Result<()>;
	async fn control_run(&mut self, id: Uuid, action: RunControlAction) -> Result<RunInspection>;
	async fn finish(self: Box<Self>, result: Result<RunInspection>) -> Result<RunInspection>;
}
#[async_trait]
pub trait RunControlRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RunControlScope + '_>>;
	fn notify(&self);
}
