//! A subject's Registry writes run on its Access transaction, so the
//! `registry.create` decision, the idempotency reservation, the definition, its
//! private documents and the event commit together or not at all.
use super::sql::SqlScope;
use crate::apps::registry::models::{RegistryRequest, transaction_records};
use crate::authorization::access::Access;
use aidash_application::{
	Result,
	ports::registry::{
		DefinitionLookup, DefinitionWriter, PrivateKnowledgeRead, PrivateKnowledgeScope,
		RegistrationScope,
	},
};
use aidash_domain::registry::Entry;
use async_trait::async_trait;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::Model;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, IntoValue, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct TenantRegistration<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) store: &'a crate::store::Store,
}

#[async_trait]
impl DefinitionLookup for TenantRegistration<'_> {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		SqlScope(&mut self.access.tx).definition(id, version).await
	}
	async fn agent_plugin(
		&mut self,
		digest: &str,
	) -> Result<Option<aidash_application::ports::bindings::StoredAgentPlugin>> {
		SqlScope(&mut self.access.tx).agent_plugin(digest).await
	}
	async fn binding_installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> Result<()> {
		SqlScope(&mut self.access.tx)
			.binding_installation(projection)
			.await
	}
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		SqlScope(&mut self.access.tx)
			.executor_kind(id, version)
			.await
	}
}

#[async_trait]
impl DefinitionWriter for TenantRegistration<'_> {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		SqlScope(&mut self.access.tx).insert_definition(entry).await
	}
}

#[async_trait]
impl RegistrationScope for TenantRegistration<'_> {
	async fn assign_id(&mut self, entry: &mut Entry, key: Option<Uuid>) -> Result<()> {
		let request = serde_json::to_value(&*entry)?;
		if entry.id.is_empty() {
			entry.id = Uuid::now_v7().to_string();
		}
		let Some(key) = key else {
			return Ok(());
		};
		let tenant = self.access.identity.tenant.clone();
		let principal = self.access.identity.subject.clone();
		let (sql, values) = Query::insert()
			.into_table(Alias::new(RegistryRequest::table_name()))
			.columns(["tenant", "principal", "key", "request", "entity_id"].map(Alias::new))
			.values_panic([
				IntoValue::into_value(&tenant),
				IntoValue::into_value(&principal),
				IntoValue::into_value(key),
				IntoValue::into_value(request.clone()),
				IntoValue::into_value(&entry.id),
			])
			.on_conflict(
				OnConflict::columns(["tenant", "principal", "key"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		self.access
			.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		let stored = RegistryRequest::objects()
			.filter(RegistryRequest::field_tenant().eq(tenant))
			.filter(RegistryRequest::field_principal().eq(principal))
			.filter(RegistryRequest::field_key().eq(key))
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(FrameworkError::from)
			.map_err(crate::Error::from)?
			.pop()
			.ok_or_else(|| crate::Error::NotFound("registry request".into()))?;
		if stored.request.0 != request {
			return Err(aidash_application::Error::Conflict(
				"registration idempotency key reused with different input".into(),
			));
		}
		entry.id = stored.entity_id;
		Ok(())
	}
	async fn authorize(&mut self, entry: &Entry) -> Result<()> {
		let resource = self.access.resource(
			"registry",
			format!("{}/{}", entry.kind, entry.id),
			json!({"kind": entry.kind, "id": entry.id, "version": entry.version}),
		);
		self.access
			.require(&resource, "registry.create")
			.await
			.map_err(Into::into)
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.store
			.event(&mut self.access.tx, None, kind, payload)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}

#[async_trait]
impl PrivateKnowledgeRead for TenantRegistration<'_> {
	async fn documents(&mut self, entry: &Entry) -> Result<Option<Value>> {
		transaction_records::documents(&mut **self.access.tx, entry)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl PrivateKnowledgeScope for TenantRegistration<'_> {
	async fn insert_documents(&mut self, entry: &Entry, documents: Value) -> Result<()> {
		transaction_records::insert_documents(&mut **self.access.tx, entry, documents)
			.await
			.map_err(Into::into)
	}
}
