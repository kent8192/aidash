//! Operator persistence uses the same protected transaction and immutable staging adapter.
use super::{
	installations::NativeStaging,
	storage::{get, put},
};
use aidash_application::ports::{
	marketplace::{OperatorScope, StagingScope},
	registry::PrivateKnowledgeRead,
};
use aidash_domain::{
	identity::Principal,
	marketplace::*,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr, TableRef,
};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;
pub(crate) struct NativeOperator<'a> {
	pub(crate) staging: NativeStaging<'a>,
	pub(crate) principal: Principal,
}
#[async_trait]
impl PrivateKnowledgeRead for NativeOperator<'_> {
	async fn documents(&mut self, entry: &Entry) -> aidash_application::Result<Option<Value>> {
		self.staging.documents(entry).await
	}
}
#[async_trait]
impl StagingScope for NativeOperator<'_> {
	fn actor(&self) -> &str {
		self.staging.actor()
	}
	fn allocate_entry_id(&mut self) -> Uuid {
		self.staging.allocate_entry_id()
	}
	async fn persist_revision(
		&mut self,
		installation: &Installation,
		revision: &Revision,
	) -> aidash_application::Result<()> {
		self.staging.persist_revision(installation, revision).await
	}
	async fn insert_documents(
		&mut self,
		entry: &Entry,
		documents: Value,
	) -> aidash_application::Result<()> {
		self.staging.insert_documents(entry, documents).await
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		provenance: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		self.staging.save_provenance(entry, provenance).await
	}
	async fn copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> aidash_application::Result<BTreeSet<ConsentEdge>> {
		self.staging.copy_provenance(entry, tenant).await
	}
	async fn save_copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
		provenance: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		self.staging
			.save_copy_provenance(entry, tenant, provenance)
			.await
	}
	async fn installed_event(&mut self, payload: Value) -> aidash_application::Result<()> {
		self.staging.installed_event(payload).await
	}
}
#[async_trait]
impl OperatorScope for NativeOperator<'_> {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn load_tenant(&mut self, tenant: &str) -> aidash_application::Result<()> {
		crate::authorization::Authorization::load(self.staging.tx, tenant)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn distribution_lock(&mut self, exclusive: bool) -> aidash_application::Result<()> {
		crate::apps::marketplace::services::storage::lock(self.staging.tx, exclusive)
			.await
			.map_err(Into::into)
	}
	async fn compatibility(&mut self) -> aidash_application::Result<Option<Compatibility>> {
		get(self.staging.tx, "marketplace_gate", "v1")
			.await
			.map_err(Into::into)
	}
	async fn save_compatibility(
		&mut self,
		state: &Compatibility,
	) -> aidash_application::Result<()> {
		put(self.staging.tx, "marketplace_gate", "v1", state)
			.await
			.map_err(Into::into)
	}
	async fn enable_writer(&mut self) -> aidash_application::Result<()> {
		crate::apps::marketplace::services::storage::writer(self.staging.tx)
			.await
			.map_err(Into::into)
	}
	async fn installation(&mut self, id: &str) -> aidash_application::Result<Option<Installation>> {
		get(self.staging.tx, "marketplace_installations", id)
			.await
			.map_err(Into::into)
	}
	async fn revision(&mut self, id: &str, revision: i64) -> aidash_application::Result<Revision> {
		crate::apps::marketplace::services::installations::revision(self.staging.tx, id, revision)
			.await
			.map_err(Into::into)
	}
	async fn approved(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
	) -> aidash_application::Result<bool> {
		super::installations::approved(self.staging.tx, tenant, reference)
			.await
			.map_err(Into::into)
	}
	async fn set_approval(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
		expected_revision: i64,
		enabled: bool,
	) -> aidash_application::Result<()> {
		crate::authorization::catalog::set_in(
			self.staging.tx,
			tenant,
			reference,
			expected_revision,
			enabled,
			"operator",
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn save_installation(
		&mut self,
		installation: &Installation,
	) -> aidash_application::Result<()> {
		put(
			self.staging.tx,
			"marketplace_installations",
			&installation.id,
			installation,
		)
		.await
		.map_err(Into::into)
	}
	async fn event(&mut self, kind: &str, payload: Value) -> aidash_application::Result<()> {
		self.staging
			.store
			.event(self.staging.tx, None, kind, payload)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn adoption_replay(&mut self, key: &str) -> aidash_application::Result<Option<Value>> {
		get(self.staging.tx, "marketplace_requests", key)
			.await
			.map_err(Into::into)
	}
	async fn remember_adoption(
		&mut self,
		key: &str,
		record: Value,
	) -> aidash_application::Result<()> {
		put(self.staging.tx, "marketplace_requests", key, &record)
			.await
			.map_err(Into::into)
	}
	async fn raw_definition(&mut self, reference: &EntityRef) -> aidash_application::Result<Entry> {
		super::definitions::raw(self.staging.tx, reference)
			.await
			.map_err(Into::into)
	}
	async fn effective_legacy(
		&mut self,
		reference: &EntityRef,
	) -> aidash_application::Result<Entry> {
		super::storage::effective_legacy(self.staging.tx, &reference.id, &reference.version)
			.await
			.map_err(Into::into)
	}
	async fn package_record(
		&mut self,
		reference: &EntityRef,
	) -> aidash_application::Result<Option<(String, String)>> {
		let record: Option<(String, String)> = {
			let query_bind_1 = &reference.id;
			let query_bind_2 = &reference.version;
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("manifest_source"), Alias::new("digest")])
					.from(Alias::new("packages"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND version=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["manifest_source", "digest"])
			.fetch_optional(&mut **self.staging.tx)
			.await?
		};
		Ok(record)
	}
	async fn legacy_config(
		&mut self,
		reference: &EntityRef,
	) -> aidash_application::Result<Option<Value>> {
		let config: Option<Value> = {
			let query_bind_1 = &reference.id;
			let query_bind_2 = &reference.version;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("config"))
					.from(Alias::new("installations"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND version=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **self.staging.tx)
			.await?
		};
		Ok(config)
	}
	async fn provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> aidash_application::Result<BTreeSet<ConsentEdge>> {
		crate::apps::marketplace::services::installations::provenance(
			self.staging.tx,
			entry,
			tenant,
		)
		.await
		.map_err(Into::into)
	}
	async fn version(&mut self, key: &str) -> aidash_application::Result<Option<Version>> {
		get(self.staging.tx, "marketplace_versions", key)
			.await
			.map_err(Into::into)
	}
	async fn insert_version(
		&mut self,
		version: &Version,
		audience: &Audience,
	) -> aidash_application::Result<()> {
		super::publication::insert_in(self.staging.tx, version, audience)
			.await
			.map_err(Into::into)
	}
	async fn revision_documents(
		&mut self,
		tenant: &str,
		offset: usize,
		limit: u64,
	) -> aidash_application::Result<Vec<(Value, Value)>> {
		let rows: Vec<(Value, Value)> = crate::database::native::query_as(
			&Query::select()
				.expr_as(
					Expr::col((Alias::new("i"), Alias::new("document"))),
					Alias::new("installation_document"),
				)
				.expr_as(
					Expr::col((Alias::new("r"), Alias::new("document"))),
					Alias::new("revision_document"),
				)
				.from_as(Alias::new("marketplace_installations"), Alias::new("i"))
				.join(
					JoinType::InnerJoin,
					TableRef::table_alias(Alias::new("marketplace_revisions"), Alias::new("r")),
					Expr::cust("r.installation=i.key"),
				)
				.and_where(
					Expr::col((Alias::new("i"), Alias::new("tenant"))).eq(Expr::value(tenant)),
				)
				.order_by((Alias::new("i"), Alias::new("key")), Order::Asc)
				.order_by((Alias::new("r"), Alias::new("revision")), Order::Asc)
				.limit(limit)
				.offset(offset as u64)
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["installation_document", "revision_document"])
		.fetch_all(&mut **self.staging.tx)
		.await?;
		Ok(rows)
	}
}
