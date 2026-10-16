//! Native adapters for Registry scopes; business admission lives in application.
use super::models::{records, transaction_records};
use crate::apps::execution::models::event_records;
use crate::{
	Result,
	registry::{AgentPage, Entry, Search},
};
use aidash_application::ports::registry::{
	DefinitionDocument, DefinitionLookup, DefinitionWriter, RegistrationScope, RegistryRead,
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{
	AtomicTransaction, DatabaseConnection, DatabaseConnectionLease, OrmExecutor,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) mod bindings;
pub(crate) mod foreign;
pub(crate) mod sql;

#[derive(Clone)]
pub struct Registry {
	pub db: DatabaseConnection,
	_lease: DatabaseConnectionLease,
	pool: crate::database::native::Pool,
	node_id: String,
	provider_credentials: bool,
}
impl Registry {
	pub fn with_provider_credentials(mut self, configured: bool) -> Self {
		self.provider_credentials = configured;
		self
	}
	pub(crate) fn validation(&self) -> aidash_application::registry::DefinitionValidation {
		crate::bootstrap::registry_validation().with_provider_credentials(self.provider_credentials)
	}
	fn scope<'a, E: OrmExecutor>(&'a self, db: &'a mut E, tenant: &'a str) -> OrmScope<'a, E> {
		OrmScope {
			db,
			node: &self.node_id,
			tenant,
			principal: "",
		}
	}
	/// Materializes the built-in definitions in every existing Tenant's
	/// Registry (ADR 0033). Byte-identical rows are left as they are.
	pub async fn seed_system(&self) -> Result<()> {
		use reinhardt::query::{Alias, Order, PostgresQueryBuilder, Query, QueryStatementBuilder};
		// A Tenant exists once its policy bundle does.
		let tenants: Vec<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("authorization_bundles"))
				.order_by(Alias::new("tenant"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&self.pool)
		.await?;
		for tenant in tenants {
			self.seed_tenant(&tenant).await?;
		}
		Ok(())
	}
	/// Materializes the built-in definitions in one Tenant's Registry.
	pub async fn seed_tenant(&self, tenant: &str) -> Result<()> {
		self.db
			.atomic(async |tx| {
				Ok(aidash_application::registry::system::seed(
					&mut self.scope(tx, tenant),
					&self.validation(),
					&self.node_id,
				)
				.await?)
			})
			.await
	}
	pub fn new(pool: impl Into<crate::database::native::Pool>, node_id: &str) -> Result<Self> {
		let pool = pool.into();
		let lease = DatabaseConnectionLease::register(pool.connection())?;
		Ok(Self {
			db: lease.handle(),
			_lease: lease,
			pool,
			node_id: node_id.into(),
			provider_credentials: false,
		})
	}
	pub async fn get(&self, tenant: &str, id: &str, version: &str) -> Result<Entry> {
		let mut db = self.db;
		Ok(self.scope(&mut db, tenant).definition(id, version).await?)
	}
	pub(crate) async fn get_for_run(
		&self,
		tenant: &str,
		run: impl Into<aidash_domain::RunMetadata>,
		id: &str,
		version: &str,
	) -> Result<Entry> {
		let mut db = self.db;
		Ok(aidash_application::registry::get_for_run(
			&mut self.scope(&mut db, tenant),
			run,
			id,
			version,
		)
		.await?)
	}
	pub async fn list(&self, tenant: &str, search: &Search) -> Result<Vec<Entry>> {
		let mut db = self.db;
		Ok(aidash_application::registry::list(&mut self.scope(&mut db, tenant), search).await?)
	}
	pub async fn legacy_agents(
		&self,
		tenant: &str,
		search: &Search,
		offset: u64,
	) -> Result<AgentPage> {
		let mut db = self.db;
		Ok(aidash_application::registry::legacy_agents(
			&mut self.scope(&mut db, tenant),
			search,
			offset,
		)
		.await?)
	}
	pub async fn register(&self, tenant: &str, entry: Entry) -> Result<Entry> {
		self.db
			.atomic(async |tx| {
				Ok(aidash_application::registry::register(
					&mut self.scope(tx, tenant),
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
		tenant: &str,
		principal: &str,
		entry: Entry,
		key: Option<Uuid>,
	) -> Result<Entry> {
		self.db
			.atomic(async |tx| {
				Ok(aidash_application::registry::register(
					&mut OrmScope {
						principal,
						..self.scope(tx, tenant)
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
	pub async fn validate_references(&self, tenant: &str, entry: &Entry) -> Result<()> {
		let mut db = self.db;
		validate_references_with(&mut db, tenant, entry, &self.node_id, &self.validation()).await
	}
}

/// One Tenant's Registry on a framework connection or transaction.
pub(crate) struct OrmScope<'a, E: OrmExecutor> {
	db: &'a mut E,
	node: &'a str,
	tenant: &'a str,
	/// The principal whose idempotency keys a tracked registration reserves.
	principal: &'a str,
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
			records::definition(self.db, self.tenant, id, version)
				.await?
				.metadata
				.0,
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
		records::insert_definition(self.db, self.tenant, entry)
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
		records::assign_id(self.db, self.tenant, self.principal, entry, key)
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
impl RegistryRead for OrmScope<'_, DatabaseConnection> {
	async fn definitions(
		&mut self,
		kind: Option<&str>,
		offset: usize,
		limit: Option<usize>,
	) -> aidash_application::Result<Vec<DefinitionDocument>> {
		Ok(records::definitions(*self.db, self.tenant, kind, offset, limit)
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
		crate::apps::execution::generation::models::is_generated_agent(
			*self.db,
			self.tenant,
			id,
			version,
		)
			.await
			.map_err(Into::into)
	}
}
/// One Tenant's Registry on a caller-owned transaction.
pub(crate) struct NativeScope<'a>(
	pub(crate) &'a mut dyn TransactionExecutor,
	/// The Tenant whose Registry this scope reads and writes.
	pub(crate) &'a str,
);
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
			transaction_records::definition(self.0, self.1, id, version)
				.await?
				.metadata
				.into_inner(),
		)?)
	}
}
#[async_trait]
impl DefinitionWriter for NativeScope<'_> {
	async fn insert_definition(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		transaction_records::insert_definition(self.0, self.1, entry)
			.await
			.map_err(Into::into)
	}
}

pub(crate) async fn validate_references_with<E: OrmExecutor>(
	db: &mut E,
	tenant: &str,
	entry: &Entry,
	node: &str,
	validation: &aidash_application::registry::DefinitionValidation,
) -> Result<()> {
	Ok(aidash_application::registry::validate_references(
		&mut OrmScope {
			db,
			node,
			tenant,
			principal: "",
		},
		validation,
		entry,
		node,
	)
	.await?)
}
pub(crate) async fn register_in(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	entry: &Entry,
	node: &str,
	validation: &aidash_application::registry::DefinitionValidation,
) -> Result<bool> {
	Ok(aidash_application::registry::register_definition(
		&mut sql::SqlScope(tx, tenant),
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
		records::documents(self.db, self.tenant, entry)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl PrivateKnowledgeScope for OrmScope<'_, AtomicTransaction> {
	async fn insert_documents(
		&mut self,
		entry: &Entry,
		documents: Value,
	) -> aidash_application::Result<()> {
		records::insert_documents(self.db, self.tenant, entry, documents)
			.await
			.map_err(Into::into)
	}
}
impl Registry {
	pub(crate) async fn register_personal(
		&self,
		tenant: &str,
		principal: &str,
		draft: aidash_application::registry::personal::PersonalDraft,
		key: Uuid,
	) -> Result<Entry> {
		let mut db = self.db;
		let registration = aidash_application::registry::personal::prepare(
			&mut self.scope(&mut db, tenant),
			&self.validation(),
			draft,
			&self.node_id,
		)
		.await?;
		self.db
			.atomic(async |tx| {
				Ok(aidash_application::registry::personal::register(
					&mut OrmScope {
						principal,
						..self.scope(tx, tenant)
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
pub(crate) async fn private_documents(
	db: &DatabaseConnection,
	tenant: &str,
	entry: &Entry,
) -> Result<Value> {
	let mut connection = *db;
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
				&mut connection,
				tenant,
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
					db: &mut connection,
					node,
					tenant,
					principal: "",
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
			db: &mut connection,
			node: "",
			tenant,
			principal: "",
		},
		entry,
	)
	.await?)
}
