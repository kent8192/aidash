//! Home disclosure composes current authority, durable journal and admitted provider effects.
use crate::{
	Result,
	ports::{
		EmbeddingProvider, VectorIndex,
		generation::dispatch::{GenerationDispatchRepository, GenerationDispatchSettlement},
		semantic::remote_journal::JournalRepository,
	},
	semantic::retrieval::PreparedSearch,
};
use aidash_domain::{
	Message, Task,
	federation::execution::{Description, home::HomeBinding},
	generation::{
		dispatch::Input,
		remote::{Reserved, Usage},
	},
	registry::AgentConfig,
	semantic::{
		indexing::IndexingSpec,
		remote::{Operation, Receipt},
		results::SearchResult,
		retrieval::Search,
	},
};
use async_trait::async_trait;
use uuid::Uuid;
#[async_trait]
pub trait SemanticSearchScope: Send {
	async fn admission_binding(&mut self, grant: Uuid) -> Result<Option<HomeBinding>>;
	/// Select worker audit context and the grant for all inherited source reads.
	fn disclosure_mode(&mut self, grant: Uuid) -> Result<()>;
	async fn disclosure_task(&mut self, task: Uuid) -> Result<Task>;
	async fn input_message(&mut self, workspace: Uuid, message: Uuid) -> Result<Message>;
	/// Retain the shared index lock, decoding native configuration defaults.
	async fn index_spec(&mut self, workspace: Uuid) -> Result<IndexingSpec>;
	async fn prepare_search(
		&mut self,
		workspace: Uuid,
		input: &Search,
		controls: &AgentConfig,
	) -> Result<PreparedSearch>;
	async fn reserve_home(&mut self, usage: &Usage) -> Result<Vec<Reserved>>;
	async fn finish_search(
		&mut self,
		prepared: PreparedSearch,
		vector: &[f32],
	) -> Result<SearchResult>;
	async fn native_stamp(
		&mut self,
		binding: &aidash_domain::semantic::remote::Binding,
	) -> Result<Option<String>> {
		if binding.native().is_some() {
			return Err(crate::Error::RemoteSemantic(
				aidash_domain::semantic::Failure::Configuration,
			));
		}
		Ok(None)
	}
	async fn native_context(
		&mut self,
		_: &str,
		binding: &aidash_domain::semantic::remote::Binding,
		_: &Operation,
		_: usize,
	) -> Result<Option<aidash_domain::semantic::remote::NativeContext>> {
		if binding.native().is_some() {
			return Err(crate::Error::RemoteSemantic(
				aidash_domain::semantic::Failure::Configuration,
			));
		}
		Ok(None)
	}
	/// Persist only the native dependencies retained after fitting the full receipt.
	async fn record_native_context(
		&mut self,
		_: &aidash_domain::semantic::remote::Binding,
		_: &Operation,
		_: &aidash_domain::semantic::remote::NativeContext,
	) -> Result<()> {
		Err(crate::Error::RemoteSemantic(
			aidash_domain::semantic::Failure::Configuration,
		))
	}
	/// Finish the original credential transaction and durable audit before a fresh authority lease.
	async fn finish(self, result: Result<Receipt>) -> Result<Receipt>;
}
#[async_trait]
pub trait SemanticSearchRepository: Send + Sync {
	type Scope: SemanticSearchScope;
	fn home_node_id(&self) -> &str;
	fn journal(&self) -> &dyn JournalRepository;
	fn dispatch(&self) -> &dyn GenerationDispatchRepository;
	fn settlement(&self) -> &dyn GenerationDispatchSettlement;
	fn embedding(&self) -> &dyn EmbeddingProvider;
	fn vector(&self) -> &dyn VectorIndex;
	/// Reconstruct current original-credential authority and retain all source locks.
	async fn description(&self, node: &str, grant: Uuid) -> Result<(Self::Scope, Description)>;
	/// Preserve the current peer check and typed bounded response under the same source lease.
	async fn verify_operation(&self, node: &str, operation: &Operation) -> Result<bool>;
	async fn receiver_reservations(&self, node: &str, input: &Input) -> Result<Vec<Reserved>>;
}
