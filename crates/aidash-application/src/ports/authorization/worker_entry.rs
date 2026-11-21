//! Opaque authority scopes retain the adapter's transaction and RAII ownership.
use crate::{Result, capabilities::sessions::Initialization};
use aidash_domain::{Run, RunMetadata, registry::AgentConfig};
use async_trait::async_trait;
#[async_trait]
pub trait WorkerEntryRepository<S: Send>: Send + Sync {
	async fn receiver_lease(&self, run: &RunMetadata) -> Result<Option<(S, AgentConfig)>>;
	async fn local_lease(&self, run: &RunMetadata, durable_audit: bool) -> Result<Option<S>>;
	async fn authorize(
		&self,
		scope: &mut S,
		run: &RunMetadata,
		read_context: bool,
	) -> Result<AgentConfig>;
	async fn scratch_space(&self, scope: &mut S, run: &Run) -> Result<uuid::Uuid>;
	async fn initialize(&self, scope: &S, run: &Run, agent: &AgentConfig)
	-> Result<Initialization>;
	/// The same for a Run executing here for another Home: bind its receiving Working Area and
	/// pin the Agent version's Reference set into it (ADR 0139).
	async fn initialize_remote(
		&self,
		scope: &S,
		run: &Run,
		agent: &AgentConfig,
	) -> Result<Initialization>;
}
