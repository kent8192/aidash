//! Native adapters for Registry scopes; business admission lives in application.
use super::models::{records, transaction_records};
use crate::apps::execution::models::event_records;
use crate::database::native::Pool;
use crate::{
	Result,
	registry::{Entry, Search},
};
use aidash_application::ports::registry::{
	DefinitionDocument, DefinitionLookup, DefinitionWriter, RegistrationScope, RegistryRead,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::tenancy::TenantName;
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::OrmExecutor;
use serde_json::Value;
use uuid::Uuid;
pub mod agent_plugins;
pub(crate) mod bindings;
pub(crate) mod requests;
pub(crate) mod sql;
pub(crate) mod versions;

/// The scope of one Tenant's Registry (ADR 0029).
fn scope(tenant: &TenantName) -> TenancyScope {
	TenancyScope::Tenant(tenant.clone())
}

#[derive(Clone)]
pub struct Registry {
	pool: Pool,
	node_id: String,
	provider_credentials: bool,
	plan_connections: bool,
	/// The Deployment's permitted private origins; absent means the settings'.
	keyless: Option<aidash_application::provider_access::KeylessAccess>,
	tenant_secrets: bool,
}
impl Registry {
	/// Share the Store's permitted origins, which tests extend after startup.
	pub fn with_keyless_origins(
		mut self,
		origins: aidash_application::provider_access::KeylessAccess,
	) -> Self {
		self.keyless = Some(origins);
		self
	}
	/// Whether definitions may name Tenant Secrets (ADR 0051).
	pub fn with_tenant_secrets(mut self, enabled: bool) -> Self {
		self.tenant_secrets = enabled;
		self
	}
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
		let validation = crate::bootstrap::registry_validation()
			.with_provider_credentials(self.provider_credentials)
			.with_plan_connections(self.plan_connections)
			.with_tenant_secrets(self.tenant_secrets);
		match &self.keyless {
			Some(origins) => validation.with_keyless_origins(origins.clone()),
			None => validation,
		}
	}
	/// Writes the built-in definitions into one Tenant's Registry, as Tenant
	/// creation does.
	pub async fn seed(&self, tenant: &TenantName) -> Result<()> {
		self.pool
			.atomic(&scope(tenant), async |tx| {
				seed_tenant(tx, &self.node_id, tenant).await
			})
			.await
	}
	/// Re-seeds the built-in definitions into every Tenant's Registry, one
	/// Tenant per transaction (ADR 0033).
	pub async fn seed_system(&self) -> Result<()> {
		let listing = TenancyScope::Deployment(DeploymentScopeReason::BuiltInSeeding);
		let tenants = self
			.pool
			.atomic(&listing, async |tx| {
				// A Tenant restored within its grace period keeps working with
				// current definitions, but a purged one must not be given back
				// the rows its purge removed.
				let names = crate::apps::identity::models::Tenant::names(tx).await?;
				let purged = crate::apps::identity::repositories::tenant_deletions::purged_of(
					tx,
					names.iter().map(|name| name.to_string()).collect(),
				)
				.await?;
				Ok(names
					.into_iter()
					.filter(|name| !purged.contains(name.as_str()))
					.collect::<Vec<_>>())
			})
			.await?;
		for tenant in tenants {
			self.pool
				.atomic(&scope(&tenant), async |tx| {
					seed_tenant(tx, &self.node_id, &tenant).await
				})
				.await?;
		}
		Ok(())
	}
	pub fn new(pool: impl Into<Pool>, node_id: &str) -> Result<Self> {
		Ok(Self {
			pool: pool.into(),
			node_id: node_id.into(),
			provider_credentials: false,
			plan_connections: false,
			keyless: None,
			tenant_secrets: false,
		})
	}
	pub async fn get(&self, tenant: &TenantName, id: &str, version: &str) -> Result<Entry> {
		self.pool
			.atomic(&scope(tenant), async |tx| {
				Ok(OrmScope::new(tx, &self.node_id, tenant)
					.definition(id, version)
					.await?)
			})
			.await
	}
	pub(crate) async fn get_for_run(
		&self,
		tenant: &TenantName,
		run: impl Into<aidash_domain::RunMetadata>,
		id: &str,
		version: &str,
	) -> Result<Entry> {
		let run = run.into();
		self.pool
			.atomic(&scope(tenant), async |tx| {
				Ok(aidash_application::registry::get_for_run(
					&mut OrmScope::new(tx, &self.node_id, tenant),
					run,
					id,
					version,
				)
				.await?)
			})
			.await
	}
	pub async fn list(&self, tenant: &TenantName, search: &Search) -> Result<Vec<Entry>> {
		self.pool
			.atomic(&scope(tenant), async |tx| {
				Ok(aidash_application::registry::list(
					&mut OrmScope::new(tx, &self.node_id, tenant),
					search,
				)
				.await?)
			})
			.await
	}
	pub async fn register(&self, tenant: &TenantName, entry: Entry) -> Result<Entry> {
		self.pool
			.atomic(&scope(tenant), async |tx| {
				Ok(aidash_application::registry::register(
					&mut OrmScope::new(tx, &self.node_id, tenant),
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
	pub async fn validate_references(&self, tenant: &TenantName, entry: &Entry) -> Result<()> {
		self.pool
			.atomic(&scope(tenant), async |tx| {
				validate_references_with(tx, tenant, entry, &self.node_id, &self.validation()).await
			})
			.await
	}
	/// The private documents an Agent's definition carries.
	pub(crate) async fn private_documents(
		&self,
		tenant: &TenantName,
		entry: &Entry,
	) -> Result<Value> {
		self.pool
			.atomic(&scope(tenant), async |tx| {
				private_documents(tx, tenant, entry).await
			})
			.await
	}
}

pub(crate) struct OrmScope<'a, E: OrmExecutor> {
	db: &'a mut E,
	node: &'a str,
	tenant: &'a TenantName,
}
impl<'a, E: OrmExecutor> OrmScope<'a, E> {
	pub(crate) fn new(db: &'a mut E, node: &'a str, tenant: &'a TenantName) -> Self {
		Self { db, node, tenant }
	}
}
#[async_trait]
impl<E: OrmExecutor> DefinitionLookup for OrmScope<'_, E> {
	async fn definition(&mut self, id: &str, version: &str) -> aidash_application::Result<Entry> {
		Ok(serde_json::from_value(
			records::definition(self.db, self.tenant, id, version)
				.await?
				.metadata
				.0,
		)?)
	}
	async fn agent_plugin(
		&mut self,
		digest: &str,
	) -> aidash_application::Result<Option<aidash_application::ports::bindings::StoredAgentPlugin>>
	{
		agent_plugins::get_of(self.db, self.tenant.as_str(), digest)
			.await
			.map_err(Into::into)
	}
	async fn binding_installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> aidash_application::Result<()> {
		bindings::installation(self.db, projection)
			.await
			.map_err(Into::into)
	}
	async fn tenant_secret_hosts(
		&mut self,
		name: &str,
	) -> aidash_application::Result<Option<Vec<String>>> {
		crate::apps::identity::repositories::tenant_secrets::active_hosts_in(
			self.db,
			self.tenant.as_str(),
			name,
		)
		.await
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
		if key.is_some() {
			return Err(aidash_application::Error::Forbidden);
		}
		records::assign_id(self.db, self.tenant, "", entry, None)
			.await
			.map_err(Into::into)
	}
	async fn authorize(&mut self, _: &Entry) -> aidash_application::Result<()> {
		Err(aidash_application::Error::Forbidden)
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
		Ok(
			records::definitions(self.db, self.tenant, kind, offset, limit)
				.await?
				.into_iter()
				.map(|row| DefinitionDocument {
					id: row.id,
					version: row.version,
					metadata: row.metadata.0,
				})
				.collect(),
		)
	}
	async fn generated(&mut self, id: &str, version: &str) -> aidash_application::Result<bool> {
		crate::apps::execution::generation::models::is_generated_agent(
			self.db,
			self.tenant,
			id,
			version,
		)
		.await
		.map_err(Into::into)
	}
}
/// Reads and writes the Registry of the transaction's Tenant scope.
pub(crate) struct NativeScope<'a>(pub(crate) &'a mut dyn TransactionExecutor);
#[async_trait]
impl DefinitionLookup for NativeScope<'_> {
	async fn agent_plugin(
		&mut self,
		digest: &str,
	) -> aidash_application::Result<Option<aidash_application::ports::bindings::StoredAgentPlugin>>
	{
		agent_plugins::get(self.0, digest).await.map_err(Into::into)
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
	async fn tenant_secret_hosts(
		&mut self,
		name: &str,
	) -> aidash_application::Result<Option<Vec<String>>> {
		crate::apps::identity::repositories::tenant_secrets::active_hosts(self.0, name).await
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

/// Writes the built-in definitions into one Tenant's Registry in the caller's
/// transaction, at Tenant creation and at startup. Changed bytes fail.
pub(crate) async fn seed_tenant<E: OrmExecutor>(
	db: &mut E,
	node: &str,
	tenant: &TenantName,
) -> Result<()> {
	Ok(aidash_application::registry::system::seed(
		&mut OrmScope::new(db, node, tenant),
		&crate::bootstrap::registry_validation(),
		node,
	)
	.await?)
}

pub(crate) async fn validate_references_with<E: OrmExecutor>(
	db: &mut E,
	tenant: &TenantName,
	entry: &Entry,
	node: &str,
	validation: &aidash_application::registry::DefinitionValidation,
) -> Result<()> {
	Ok(aidash_application::registry::validate_references(
		&mut OrmScope::new(db, node, tenant),
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

use aidash_application::ports::registry::PrivateKnowledgeRead;
#[async_trait]
impl<E: OrmExecutor> PrivateKnowledgeRead for OrmScope<'_, E> {
	async fn documents(&mut self, entry: &Entry) -> aidash_application::Result<Option<Value>> {
		records::documents(self.db, self.tenant, entry)
			.await
			.map_err(Into::into)
	}
}
async fn private_documents<E: OrmExecutor>(
	connection: &mut E,
	tenant: &TenantName,
	entry: &Entry,
) -> Result<Value> {
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
				&mut OrmScope::new(&mut *connection, node, tenant),
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
		&mut OrmScope::new(&mut *connection, "", tenant),
		entry,
	)
	.await?)
}
