//! A Run inspection holds one current disclosure transaction through its bounded projection.
use crate::Result;
use aidash_domain::{
	RawRun, RunInspection, RunMetadata, invocation::InvocationSummary, policy::Resource,
};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;
#[derive(Debug)]
pub struct RunDetails {
	pub run: RunInspection,
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
	/// Preserve the original 100-row created_at/idempotency-key page and bounded JSON preview.
	async fn invocations(&mut self, id: Uuid, offset: u64) -> Result<Vec<InvocationSummary>>;
	async fn memory(&mut self, run: &RunMetadata)
	-> Result<Option<aidash_domain::memory::Binding>>;
	async fn media_input_routes(&mut self, run: &RunMetadata) -> Result<Vec<Vec<String>>>;
	async fn finish(self: Box<Self>, result: Result<RunDetails>) -> Result<RunDetails>;
}
#[async_trait]
pub trait RunDetailsRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RunDetailsScope + '_>>;
}
