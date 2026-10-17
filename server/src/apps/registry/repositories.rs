//! Native adapters for Registry scopes; business admission lives in application.
use super::models::{records, transaction_records};
use crate::apps::execution::models::event_records;
use crate::database::native::Pool;
use crate::{
	Result,
	registry::{AgentPage, Entry, Search},
};
use aidash_application::ports::registry::{
	DefinitionDocument, DefinitionLookup, DefinitionWriter, RegistrationScope, RegistryRead,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{AtomicTransaction, OrmExecutor};
use serde_json::Value;
use uuid::Uuid;
pub(crate) mod bindings;
pub(crate) mod foreign;
pub(crate) mod sql;

/// Every Tenant shares the Deployment's single Registry until #265.
const SHARED: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::SharedRegistry);

#[derive(Clone)]
pub struct Registry {
	pool: Pool,
	node_id: String,
	provider_credentials: bool,
	plan_connections: bool,
}
impl Registry {
	pub fn with_provider_credentials(mut self, configured: bool) -> Self {
		self.provider_credentials = configured;
		self
	}
	/// Whether `chatgpt` model definitions may be registered (ADR 0007).
	pub fn with_plan_connections(mut self, enabled: bool) -> Self {
		self.plan_connections = enabled;
		self
	}
	pub(crate) fn validation(&self) -> aidash_application::registry::DefinitionValidation {
		crate::bootstrap::registry_validation()
			.with_provider_credentials(self.provider_credentials)
			.with_plan_connections(self.plan_connections)
	}
	pub async fn seed_system(&self) -> Result<()> {
		let seeding = TenancyScope::Deployment(DeploymentScopeReason::BuiltInSeeding);
		self.pool
			.atomic(&seeding, async |tx| {
				Ok(aidash_application::registry::system::seed(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					&self.validation(),
					&self.node_id,
				)
				.await?)
			})
			.await
	}
	pub fn new(pool: impl Into<Pool>, node_id: &str) -> Result<Self> {
		Ok(Self {
			pool: pool.into(),
			node_id: node_id.into(),
			provider_credentials: false,
			plan_connections: false,
		})
	}
	pub async fn get(&self, id: &str, version: &str) -> Result<Entry> {
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(OrmScope {
					db: tx,
					node: &self.node_id,
				}
				.definition(id, version)
				.await?)
			})
			.await
	}
	pub(crate) async fn get_for_run(
		&self,
		run: impl Into<aidash_domain::RunMetadata>,
		id: &str,
		version: &str,
	) -> Result<Entry> {
		let run = run.into();
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::get_for_run(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					run,
					id,
					version,
				)
				.await?)
			})
			.await
	}
	pub async fn list(&self, search: &Search) -> Result<Vec<Entry>> {
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::list(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					search,
				)
				.await?)
			})
			.await
	}
	pub async fn legacy_agents(&self, search: &Search, offset: u64) -> Result<AgentPage> {
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::legacy_agents(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					search,
					offset,
				)
				.await?)
			})
			.await
	}
	pub async fn register(&self, entry: Entry) -> Result<Entry> {
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::register(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					&self.validation(),
					entry,
					None,
					false,
					&self.node_id,
				)
				.await?)
			})
			.await
	}
	pub(crate) async fn register_with_event(
		&self,
		entry: Entry,
		key: Option<Uuid>,
	) -> Result<Entry> {
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::register(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					&self.validation(),
					entry,
					key,
					true,
					&self.node_id,
				)
				.await?)
			})
			.await
	}
	pub async fn validate_references(&self, entry: &Entry) -> Result<()> {
		self.pool
			.atomic(&SHARED, async |tx| {
				validate_references_with(tx, entry, &self.node_id, &self.validation()).await
			})
			.await
	}
	/// The private documents an Agent's definition carries.
	pub(crate) async fn private_documents(&self, entry: &Entry) -> Result<Value> {
		self.pool
			.atomic(&SHARED, async |tx| private_documents(tx, entry).await)
			.await
	}
}

pub(crate) struct OrmScope<'a, E: OrmExecutor> {
	db: &'a mut E,
	node: &'a str,
}
#[async_trait]
impl<E: OrmExecutor> DefinitionLookup for OrmScope<'_, E> {
	async fn foreign_agent(
		&mut self,
		node: &str,
		reference: &aidash_domain::registry::bindings::QualifiedRef,
	) -> aidash_application::Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		if node != self.node {
			return Err(aidash_application::Error::Forbidden);
		}
		foreign::orm(self.db, node, reference)
			.await
			.map_err(Into::into)
	}
	async fn definition(&mut self, id: &str, version: &str) -> aidash_application::Result<Entry> {
		Ok(serde_json::from_value(
			records::definition(self.db, id, version).await?.metadata.0,
		)?)
	}
	async fn binding_installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> aidash_application::Result<()> {
		bindings::installation(self.db, projection)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl<E: OrmExecutor> DefinitionWriter for OrmScope<'_, E> {
	async fn insert_definition(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		records::insert_definition(self.db, entry)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl<E: OrmExecutor + TransactionExecutor> RegistrationScope for OrmScope<'_, E> {
	async fn assign_id(
		&mut self,
		entry: &mut Entry,
		key: Option<Uuid>,
	) -> aidash_application::Result<()> {
		records::assign_id(self.db, entry, key)
			.await
			.map_err(Into::into)
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> aidash_application::Result<()> {
		event_records::append(self.db, self.node, None, kind, payload)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
#[async_trait]
impl<E: OrmExecutor> RegistryRead for OrmScope<'_, E> {
	async fn definitions(
		&mut self,
		kind: Option<&str>,
		offset: usize,
		limit: Option<usize>,
	) -> aidash_application::Result<Vec<DefinitionDocument>> {
		Ok(records::definitions(self.db, kind, offset, limit)
			.await?
			.into_iter()
			.map(|row| DefinitionDocument {
				id: row.id,
				version: row.version,
				metadata: row.metadata.0,
			})
			.collect())
	}
	async fn generated(&mut self, id: &str, version: &str) -> aidash_application::Result<bool> {
		crate::apps::execution::generation::models::is_generated_agent(self.db, id, version)
			.await
			.map_err(Into::into)
	}
}
pub(crate) struct NativeScope<'a>(pub(crate) &'a mut dyn TransactionExecutor);
#[async_trait]
impl DefinitionLookup for NativeScope<'_> {
	async fn foreign_agent(
		&mut self,
		node: &str,
		reference: &aidash_domain::registry::bindings::QualifiedRef,
	) -> aidash_application::Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		foreign::native(self.0, node, reference)
			.await
			.map_err(Into::into)
	}
	async fn binding_installation(
		&mut self,
		p: &aidash_domain::registry::Projection,
	) -> aidash_application::Result<()> {
		bindings::installation_executor(self.0, p)
			.await
			.map_err(Into::into)
	}

	async fn definition(&mut self, id: &str, version: &str) -> aidash_application::Result<Entry> {
		Ok(serde_json::from_value(
			transaction_records::definition(self.0, id, version)
				.await?
				.metadata
				.into_inner(),
		)?)
	}
}
#[async_trait]
impl DefinitionWriter for NativeScope<'_> {
	async fn insert_definition(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		transaction_records::insert_definition(self.0, entry)
			.await
			.map_err(Into::into)
	}
}

pub(crate) async fn validate_references_with<E: OrmExecutor>(
	db: &mut E,
	entry: &Entry,
	node: &str,
	validation: &aidash_application::registry::DefinitionValidation,
) -> Result<()> {
	Ok(aidash_application::registry::validate_references(
		&mut OrmScope { db, node },
		validation,
		entry,
		node,
	)
	.await?)
}
pub(crate) async fn register_in(
	tx: &mut crate::database::native::Transaction,
	entry: &Entry,
	node: &str,
	validation: &aidash_application::registry::DefinitionValidation,
) -> Result<bool> {
	Ok(aidash_application::registry::register_definition(
		&mut sql::SqlScope(tx),
		validation,
		entry,
		node,
	)
	.await?)
}

use aidash_application::ports::registry::{PrivateKnowledgeRead, PrivateKnowledgeScope};
#[async_trait]
impl<E: OrmExecutor> PrivateKnowledgeRead for OrmScope<'_, E> {
	async fn documents(&mut self, entry: &Entry) -> aidash_application::Result<Option<Value>> {
		records::documents(self.db, entry).await.map_err(Into::into)
	}
}
#[async_trait]
impl PrivateKnowledgeScope for OrmScope<'_, AtomicTransaction> {
	async fn insert_documents(
		&mut self,
		entry: &Entry,
		documents: Value,
	) -> aidash_application::Result<()> {
		records::insert_documents(self.db, entry, documents)
			.await
			.map_err(Into::into)
	}
}
impl Registry {
	pub(crate) async fn register_personal(
		&self,
		draft: aidash_application::registry::personal::PersonalDraft,
		key: Uuid,
	) -> Result<Entry> {
		let registration = self
			.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::personal::prepare(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					&self.validation(),
					draft,
					&self.node_id,
				)
				.await?)
			})
			.await?;
		self.pool
			.atomic(&SHARED, async |tx| {
				Ok(aidash_application::registry::personal::register(
					&mut OrmScope {
						db: tx,
						node: &self.node_id,
					},
					&self.validation(),
					registration,
					key,
					&self.node_id,
				)
				.await?)
			})
			.await
	}
}
async fn private_documents<E: OrmExecutor>(connection: &mut E, entry: &Entry) -> Result<Value> {
	if entry.kind == "agent" {
		let input: aidash_domain::registry::bindings::AgentBindings =
			serde_json::from_value(entry.config.clone())?;
		let node = entry
			.binding_normalization
			.as_ref()
			.ok_or_else(|| {
				crate::Error::Invalid("Agent lacks registered Binding provenance".into())
			})?
			.registry_node
			.as_str();
		let mut documents = vec![];
		for binding in input
			.bindings
			.iter()
			.filter(|b| b.kind == aidash_domain::registry::bindings::BindingKind::Source)
		{
			if binding.target.registry_node != node {
				return Err(crate::Error::Forbidden);
			}
			let source = records::definition(
				&mut *connection,
				&binding.target.id,
				&binding.target.version,
			)
			.await?;
			let source: Entry = serde_json::from_value(source.metadata.into_inner())?;
			if source.config.get("schema_version").is_none() {
				continue;
			}
			let context: aidash_domain::registry::bindings::sources::NativeContext =
				serde_json::from_value(source.config.clone())?;
			if !matches!(
				context.source,
				aidash_domain::registry::bindings::sources::NativeSource::PrivateReferences { .. }
			) {
				continue;
			}
			let value = aidash_application::registry::personal::load(
				&mut OrmScope {
					db: &mut *connection,
					node,
				},
				&source,
			)
			.await?;
			documents.extend(
				value
					.as_array()
					.ok_or_else(|| {
						crate::Error::Invalid("private Source requires a document list".into())
					})?
					.iter()
					.cloned(),
			);
		}
		return Ok(serde_json::json!(documents));
	}
	Ok(aidash_application::registry::personal::load(
		&mut OrmScope {
			db: &mut *connection,
			node: "",
		},
		entry,
	)
	.await?)
}
