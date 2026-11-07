//! A Run inspection holds one current disclosure transaction through its bounded projection.
use crate::Result;
use aidash_domain::{
	RawRun, RunInspection, RunMetadata, invocation::InvocationSummary, policy::Resource,
	run_state::RunReadsBlock,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug)]
pub struct RunDetails {
	pub provider_tool_activity: Vec<aidash_domain::tool::provider_tools::Activity>,
	pub egress_requests: Vec<aidash_domain::egress::RequestRecord>,
	pub run: RunInspection,
	/// The call the Run is paused on, only for a caller who may decline it.
	pub declinable_call: Option<aidash_domain::run_state::DeclinableCall>,
	pub invocations: Vec<InvocationSummary>,
	pub memory: Option<aidash_domain::memory::Binding>,
	pub media_input_routes: Vec<Vec<String>>,
}
#[async_trait]
pub trait RunDetailsScope: Send {
	fn node_id(&self) -> &str;
	fn set_context(&mut self, context: Value);
	async fn run(&mut self, id: Uuid) -> Result<Option<RawRun>>;
	async fn workspace(&mut self, id: Uuid) -> Result<Resource>;
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()>;
	async fn run_visible(&mut self, run: &RunInspection) -> Result<bool>;
	/// What the failed [`Self::run_visible`] found in the Run's reads, in terms
	/// this reader may see. `None` when the reader's own policy hides the Run.
	async fn run_reads_block(&mut self, run: &RunInspection) -> Result<Option<RunReadsBlock>>;
	/// Whether the caller may decline the call the paused Run stopped on: the Run's person under
	/// `run.control`, anyone else under `run.decline_call`. Decided without writing a denial.
	async fn may_decline(&mut self, _run: &RunInspection) -> Result<bool> {
		Ok(false)
	}
	/// Preserve the original 100-row created_at/idempotency-key page and bounded JSON preview.
	async fn egress_requests(
		&mut self,
		_id: Uuid,
		_offset: u64,
	) -> Result<Vec<aidash_domain::egress::RequestRecord>> {
		Ok(vec![])
	}
	async fn provider_tool_activity(
		&mut self,
		_id: Uuid,
		_offset: u64,
	) -> Result<Vec<aidash_domain::tool::provider_tools::Activity>> {
		Ok(vec![])
	}
	async fn invocations(&mut self, id: Uuid, offset: u64) -> Result<Vec<InvocationSummary>>;
	async fn memory(&mut self, run: &RunMetadata)
	-> Result<Option<aidash_domain::memory::Binding>>;
	async fn media_input_routes(&mut self, run: &RunMetadata) -> Result<Vec<Vec<String>>>;
	async fn finish(self: Box<Self>, result: Result<RunDetails>) -> Result<RunDetails>;
	/// Finish a visibility decision that read no diagnostics.
	async fn finish_visible(
		self: Box<Self>,
		result: Result<RunInspection>,
	) -> Result<RunInspection>;
}
#[async_trait]
pub trait RunDetailsRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RunDetailsScope + '_>>;
}
