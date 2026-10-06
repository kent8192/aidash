use super::*;
use crate::{
	authorization::{access::Access, identity::Actor},
	store::Store,
};

pub(crate) use crate::apps::knowledge::repositories::access::Lease;
pub(crate) use crate::apps::knowledge::repositories::storage::{history, index, schedule_point};

pub async fn configure(store: &Store, workspace: Uuid, input: ConfigureIndex) -> Result<Index> {
	aidash_application::semantic::mutations::configure(
		&crate::bootstrap::semantic_configuration_repository(store),
		workspace,
		input.expected_revision,
		input.spec.into(),
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub async fn get_index(store: &Store, actor: &Actor, workspace: Uuid) -> Result<Index> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = aidash_application::semantic::mutations::get_index(
		&mut crate::bootstrap::semantic_entries_scope(&mut lease),
		workspace,
	)
	.await
	.map(Into::into)
	.map_err(Into::into);
	lease.finish(result).await
}
pub async fn put(store: &Store, actor: &Actor, workspace: Uuid, input: PutEntry) -> Result<Entry> {
	input.validate()?;
	let mut lease = Lease::begin(store, actor).await?;
	let result = async {
		let input = input.into();
		aidash_application::semantic::mutations::validate_public_put(&input)?;
		aidash_application::semantic::mutations::put(
			&mut crate::bootstrap::semantic_entries_scope(&mut lease),
			workspace,
			input,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	.await;
	lease.finish(result).await
}

pub async fn entries(store: &Store, actor: &Actor, workspace: Uuid) -> Result<Vec<Entry>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = aidash_application::semantic::mutations::entries(
		&mut crate::bootstrap::semantic_entries_scope(&mut lease),
		workspace,
	)
	.await
	.map(|rows| rows.into_iter().map(Into::into).collect())
	.map_err(Into::into);
	lease.finish(result).await
}
pub async fn change(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	id: Uuid,
	revision: i64,
	delete: bool,
) -> Result<Entry> {
	aidash_domain::semantic::mutations::validate_revision(revision, "invalid source revision")?;
	let mut lease = Lease::begin(store, actor).await?;
	let result = aidash_application::semantic::mutations::change(
		&mut crate::bootstrap::semantic_entries_scope(&mut lease),
		workspace,
		id,
		revision,
		delete,
	)
	.await
	.map(Into::into)
	.map_err(Into::into);
	lease.finish(result).await
}
pub async fn search(
	store: &Store,
	actor: &Actor,
	workspace: Uuid,
	input: Search,
) -> Result<SearchResult> {
	let mut lease = Lease::begin(store, actor).await?;
	lease.durable();
	let result = search_in(store, &mut lease, workspace, input, None, None).await;
	lease.finish(result).await
}
pub(crate) async fn search_in(
	store: &Store,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	input: Search,
	run: Option<Uuid>,
	agent_controls: Option<&crate::registry::AgentConfig>,
) -> Result<SearchResult> {
	aidash_application::semantic::retrieval::search(
		&mut crate::bootstrap::semantic_retrieval_scope(store, lease),
		&crate::bootstrap::semantic_transport(store),
		workspace,
		&(&input).into(),
		run,
		agent_controls,
	)
	.await
	.map_err(Into::into)
}

pub(crate) use aidash_domain::semantic::indexing::content_digest;

pub async fn history_list(store: &Store, actor: &Actor, workspace: Uuid) -> Result<Vec<History>> {
	let mut lease = Lease::begin(store, actor).await?;
	let result = aidash_application::semantic::mutations::history(
		&mut crate::bootstrap::semantic_entries_scope(&mut lease),
		workspace,
	)
	.await
	.map(|rows| rows.into_iter().map(Into::into).collect())
	.map_err(Into::into);
	lease.finish(result).await
}

pub(crate) async fn context_in(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &crate::domain::Run,
	query: &str,
	budget: usize,
	agent: &crate::registry::AgentConfig,
) -> Result<Option<SearchResult>> {
	aidash_application::semantic::retrieval::context(
		&mut crate::bootstrap::semantic_retrieval_scope(store, lease),
		&crate::bootstrap::semantic_transport(store),
		run,
		query,
		budget,
		agent,
	)
	.await
	.map_err(Into::into)
}

impl Access {
	pub(crate) async fn semantic_reads_visible(&mut self, run: Uuid) -> Result<bool> {
		aidash_application::semantic::memory::reads_visible(
			&mut crate::bootstrap::semantic_memory_scope(&mut Lease::Inherited(self)),
			run,
		)
		.await
		.map_err(Into::into)
	}
}

/// Keep the authority lease through reservation, provider I/O and settlement.
pub(crate) async fn embed(
	store: &Store,
	lease: &mut Lease<'_>,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: crate::generation::embedding::Origin,
) -> Result<Vec<f32>> {
	aidash_application::semantic::embedding::invoke(
		&mut crate::bootstrap::semantic_embedding_scope(store, lease),
		&crate::bootstrap::semantic_transport(store),
		workspace,
		config,
		text,
		origin,
	)
	.await
	.map_err(Into::into)
}
