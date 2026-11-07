//! Resuming a Run borrows one transaction through grant locking, mutation, audit and commit.
use crate::Result;
use aidash_domain::{
	RawRun, RunInspection,
	identity::execution::{ExecutionGrant, ExecutionPrincipal},
	policy::Resource,
	run_state::RunReadsBlock,
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
	/// What the failed [`Self::run_visible`] found in the Run's reads, in terms
	/// this reader may see. `None` when the reader's own policy hides the Run.
	async fn run_reads_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>>;
	/// The first definition the Run uses that it may no longer use (the Use Recheck), as
	/// this caller may see it. `None` for a Run of another Home or without a Binding
	/// snapshot.
	async fn use_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>>;
	/// The resume grant remains locked FOR UPDATE until the surrounding transaction finishes.
	async fn lock_execution_grant(&mut self, id: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn update_credential(&mut self, id: Uuid, credential: Uuid) -> Result<()>;
	async fn resume_run(&mut self, id: Uuid) -> Result<RunInspection>;
	async fn finish(self: Box<Self>, result: Result<RunInspection>) -> Result<RunInspection>;
}
#[async_trait]
pub trait RunControlRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RunControlScope + '_>>;
	fn notify(&self);
}
