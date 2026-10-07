//! Compose existing native scopes and adapters without moving any transaction or RPC boundary.
use crate::apps::execution::generation::repositories::dispatch::{
	NativeDispatch, NativeSettlement,
};
use crate::apps::knowledge::{
	repositories::{access::Lease, remote_journal::Repository as Journal},
	services::service,
};
use crate::{Error as NativeError, authorization::access::Access, federation::Federation};
use aidash_application::{
	Result,
	ports::{
		EmbeddingProvider, VectorIndex,
		authorization::source::search::{SemanticSearchRepository, SemanticSearchScope},
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
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) journal: Journal<'a>,
	pub(crate) dispatch: NativeDispatch,
	pub(crate) settlement: NativeSettlement,
	pub(crate) transport: crate::apps::knowledge::repositories::postgres_vector::Transport,
}
pub(crate) struct Scope {
	runtime: Federation,
	lease: Lease<'static>,
}
impl Scope {
	fn access(&mut self) -> crate::Result<&mut Access> {
		self.lease.access().ok_or(NativeError::Forbidden)
	}
}
#[async_trait]
impl SemanticSearchRepository for Repository<'_> {
	type Scope = Scope;
	fn home_node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn journal(&self) -> &dyn JournalRepository {
		&self.journal
	}
	fn dispatch(&self) -> &dyn GenerationDispatchRepository {
		&self.dispatch
	}
	fn settlement(&self) -> &dyn GenerationDispatchSettlement {
		&self.settlement
	}
	fn embedding(&self) -> &dyn EmbeddingProvider {
		&self.transport
	}
	fn vector(&self) -> &dyn VectorIndex {
		&self.transport
	}
	async fn description(&self, node: &str, grant: Uuid) -> Result<(Self::Scope, Description)> {
		let (access, description) =
			crate::authorization::remote::description_lease(self.runtime, node, grant).await?;
		Ok((
			Scope {
				runtime: self.runtime.clone(),
				lease: Lease::Scoped(Box::new(access)),
			},
			description,
		))
	}
	async fn verify_operation(&self, node: &str, operation: &Operation) -> Result<bool> {
		crate::authorization::peer::authority_request(
			self.runtime,
			node,
			"/scoped/semantic/verify-operation",
			&serde_json::to_value(operation)?,
		)
		.await
		.map_err(Into::into)
	}
	async fn receiver_reservations(&self, node: &str, input: &Input) -> Result<Vec<Reserved>> {
		crate::authorization::peer::authority_request(
			self.runtime,
			node,
			"/scoped/usage/reserve",
			&serde_json::to_value(input)?,
		)
		.await
		.map_err(Into::into)
	}
}
#[async_trait]
impl SemanticSearchScope for Scope {
	async fn admission_binding(&mut self, grant: Uuid) -> Result<Option<HomeBinding>> {
		super::super::home_execution::binding(self.access()?, grant)
			.await
			.map(|r| r.map(Into::into))
			.map_err(Into::into)
	}
	fn disclosure_mode(&mut self, grant: Uuid) -> Result<()> {
		let access = self.access()?;
		access.worker();
		access.durable_audit = true;
		access.read_grant = Some(grant);
		Ok(())
	}
	async fn disclosure_task(&mut self, task: Uuid) -> Result<Task> {
		self.access()?.task_read(task).await.map_err(Into::into)
	}
	async fn input_message(&mut self, workspace: Uuid, message: Uuid) -> Result<Message> {
		Ok(serde_json::from_value(
			self.access()?
				.workspace_record(workspace, "message", message)
				.await?,
		)?)
	}
	async fn index_spec(&mut self, workspace: Uuid) -> Result<IndexingSpec> {
		let index = service::index(self.lease.tx(), workspace, false).await?;
		index.configuration().map(Into::into).map_err(Into::into)
	}
	async fn prepare_search(
		&mut self,
		workspace: Uuid,
		input: &Search,
		controls: &AgentConfig,
	) -> Result<PreparedSearch> {
		let store = self.runtime.store.clone();
		aidash_application::semantic::retrieval::prepare(
			&mut crate::bootstrap::semantic_retrieval_scope(&store, &mut self.lease),
			workspace,
			input,
			Some(controls),
		)
		.await
	}
	async fn reserve_home(&mut self, usage: &Usage) -> Result<Vec<Reserved>> {
		let store = self.runtime.store.clone();
		crate::generation::remote::reserve(self.access()?, &store, usage)
			.await
			.map_err(Into::into)
	}
	async fn finish_search(
		&mut self,
		prepared: PreparedSearch,
		vector: &[f32],
	) -> Result<SearchResult> {
		let store = self.runtime.store.clone();
		aidash_application::semantic::retrieval::finish(
			&crate::bootstrap::semantic_transport(&store),
			&mut crate::bootstrap::semantic_retrieval_scope(&store, &mut self.lease),
			prepared,
			vector,
			true,
		)
		.await
	}
	async fn finish(self, result: Result<Receipt>) -> Result<Receipt> {
		self.lease
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
	async fn native_stamp(
		&mut self,
		binding: &aidash_domain::semantic::remote::Binding,
	) -> Result<Option<String>> {
		if let Some(native) = binding.native() {
			Ok(Some(
				crate::apps::knowledge::services::remote_memory::stamp(
					&self.runtime.store,
					&mut self.lease,
					native,
				)
				.await?,
			))
		} else {
			Ok(None)
		}
	}
	async fn native_context(
		&mut self,
		node: &str,
		binding: &aidash_domain::semantic::remote::Binding,
		operation: &Operation,
		budget: usize,
	) -> Result<Option<aidash_domain::semantic::remote::NativeContext>> {
		if let Some(native) = binding.native() {
			Ok(Some(
				crate::apps::knowledge::services::remote_memory::retrieve(
					&self.runtime,
					&mut self.lease,
					node,
					native,
					operation,
					budget,
				)
				.await?,
			))
		} else {
			Ok(None)
		}
	}
	async fn record_native_context(
		&mut self,
		binding: &aidash_domain::semantic::remote::Binding,
		operation: &Operation,
		context: &aidash_domain::semantic::remote::NativeContext,
	) -> Result<()> {
		crate::apps::knowledge::services::remote_memory::record(
			&self.runtime.store,
			&mut self.lease,
			binding.native().ok_or(NativeError::Forbidden)?,
			operation.grant_id,
			context,
		)
		.await
		.map_err(Into::into)
	}
}
