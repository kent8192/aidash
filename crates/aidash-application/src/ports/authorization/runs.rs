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
	/// The Node this scope runs on, which decides whether a Run is local.
	fn node_id(&self) -> &str;
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
	/// What the Run's own reads say against letting it go on, without asking whether this reader
	/// may read the Run. `None` when nothing stops it (#763).
	async fn run_reads_stop(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>>;
	/// The first definition the Run uses that it may no longer use (the Use Recheck), as
	/// this caller may see it. `None` for a Run of another Home. A Run whose Binding
	/// snapshot cannot be read is checked for its own Agent only, as its next boundary is.
	async fn use_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>>;
	/// The resume grant remains locked FOR UPDATE until the surrounding transaction finishes.
	async fn lock_execution_grant(&mut self, id: Uuid) -> Result<Option<ExecutionGrant>>;
	async fn update_credential(&mut self, id: Uuid, credential: Uuid) -> Result<()>;
	async fn resume_run(&mut self, id: Uuid) -> Result<RunInspection>;
	/// Answer the call `id` is paused on as declined and let the Run go on, in this transaction.
	/// `Conflict` when the Run is no longer paused on that call.
	async fn decline_call(&mut self, id: Uuid, call: &str) -> Result<RunInspection>;
	async fn finish(self: Box<Self>, result: Result<RunInspection>) -> Result<RunInspection>;
}
#[async_trait]
pub trait RunControlRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RunControlScope + '_>>;
	fn notify(&self);
}
