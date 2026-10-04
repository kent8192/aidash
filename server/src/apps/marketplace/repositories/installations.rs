//! Immutable installation persistence under the original transaction and lock order.
use super::{
	definitions::NativeDefinitions,
	storage::{get, put},
};
use crate::{Error, Result, store::Store};
use aidash_application::ports::{
	marketplace::{InstallationRead, InstallationScope, StagingScope},
	registry::PrivateKnowledgeRead,
};
use aidash_domain::{
	marketplace::definitions::{content, key, reference},
	marketplace::{ConsentEdge, Installation, Replay, Revision},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) struct NativeStaging<'a, 'connection> {
	pub(crate) store: &'a Store,
	pub(crate) tx: &'a mut Transaction<'connection, Postgres>,
	pub(crate) actor: &'a str,
}
#[async_trait]
impl<Events: Send> InstallationRead for NativeDefinitions<'_, Events> {
	async fn installation_gate(&mut self) -> aidash_application::Result<()> {
		// Inherited transactions already hold the outer distribution lease.
		if !self.access.inherited_lease {
			crate::apps::marketplace::services::storage::lock(&mut self.access.tx, false).await?;
		}
		crate::apps::marketplace::services::storage::gate(&mut self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn selected_installation(
		&mut self,
		id: &str,
	) -> aidash_application::Result<Option<Installation>> {
		let doc: Option<Value> = {
			let query_bind_1 = &id;
			sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("document"))
					.from(Alias::new("marketplace_installations"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(key=?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await
			.map_err(crate::Error::from)?
		};
		doc.map(serde_json::from_value)
			.transpose()
			.map_err(Into::into)
	}
	fn audit_installation(&mut self, installation: Value) {
		if let Some(audit) = &mut self.access.marketplace_audit {
			audit["installation"] = installation;
		}
	}
	async fn approved(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
	) -> aidash_application::Result<bool> {
		approved(&mut self.access.tx, tenant, reference)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl<Events: Send> PrivateKnowledgeRead for NativeDefinitions<'_, Events> {
	async fn documents(&mut self, entry: &Entry) -> aidash_application::Result<Option<Value>> {
		documents(&mut self.access.tx, entry)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl PrivateKnowledgeRead for NativeStaging<'_, '_> {
	async fn documents(&mut self, entry: &Entry) -> aidash_application::Result<Option<Value>> {
		documents(self.tx, entry).await.map_err(Into::into)
	}
}
#[async_trait]
impl StagingScope for NativeStaging<'_, '_> {
	fn actor(&self) -> &str {
		self.actor
	}
	fn allocate_entry_id(&mut self) -> Uuid {
		Uuid::new_v4()
	}
	async fn persist_revision(
		&mut self,
		installation: &Installation,
		revision: &Revision,
	) -> aidash_application::Result<()> {
		persist_revision(self.tx, installation, revision)
			.await
			.map_err(Into::into)
	}
	async fn insert_documents(
		&mut self,
		entry: &Entry,
		documents: Value,
	) -> aidash_application::Result<()> {
		insert_documents(self.tx, entry, documents)
			.await
			.map_err(Into::into)
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		provenance: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		put(
			self.tx,
			"marketplace_provenance",
			&key(&reference(entry)),
			provenance,
		)
		.await
		.map_err(Into::into)
	}
	async fn copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> aidash_application::Result<BTreeSet<ConsentEdge>> {
		Ok(get(
			self.tx,
			"marketplace_provenance",
			&format!("content-{}", key(&(tenant, content(entry)))),
		)
		.await?
		.unwrap_or_default())
	}
	async fn save_copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
		provenance: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		put(
			self.tx,
			"marketplace_provenance",
			&format!("content-{}", key(&(tenant, content(entry)))),
			provenance,
		)
		.await
		.map_err(Into::into)
	}
	async fn installed_event(&mut self, payload: Value) -> aidash_application::Result<()> {
		self.store
			.event(self.tx, None, "marketplace.installed", payload)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
#[async_trait]
impl StagingScope for NativeDefinitions<'_, &Store> {
	fn actor(&self) -> &str {
		&self.access.identity.subject
	}
	fn allocate_entry_id(&mut self) -> Uuid {
		Uuid::new_v4()
	}
	async fn persist_revision(
		&mut self,
		installation: &Installation,
		revision: &Revision,
	) -> aidash_application::Result<()> {
		persist_revision(&mut self.access.tx, installation, revision)
			.await
			.map_err(Into::into)
	}
	async fn insert_documents(
		&mut self,
		entry: &Entry,
		documents: Value,
	) -> aidash_application::Result<()> {
		insert_documents(&mut self.access.tx, entry, documents)
			.await
			.map_err(Into::into)
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		provenance: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		put(
			&mut self.access.tx,
			"marketplace_provenance",
			&key(&reference(entry)),
			provenance,
		)
		.await
		.map_err(Into::into)
	}
	async fn copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> aidash_application::Result<BTreeSet<ConsentEdge>> {
		Ok(get(
			&mut self.access.tx,
			"marketplace_provenance",
			&format!("content-{}", key(&(tenant, content(entry)))),
		)
		.await?
		.unwrap_or_default())
	}
	async fn save_copy_provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
		provenance: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		put(
			&mut self.access.tx,
			"marketplace_provenance",
			&format!("content-{}", key(&(tenant, content(entry)))),
			provenance,
		)
		.await
		.map_err(Into::into)
	}
	async fn installed_event(&mut self, payload: Value) -> aidash_application::Result<()> {
		self.events
			.event(&mut self.access.tx, None, "marketplace.installed", payload)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
#[async_trait]
impl InstallationScope for NativeDefinitions<'_, &Store> {
	async fn installation_replay(
		&mut self,
		operation: &str,
		request: Uuid,
	) -> aidash_application::Result<Option<Replay>> {
		get(
			&mut self.access.tx,
			"marketplace_requests",
			&key(&(
				self.access.identity.tenant.as_str(),
				self.access.identity.subject.as_str(),
				operation,
				request,
			)),
		)
		.await
		.map_err(Into::into)
	}
	async fn remember_installation(
		&mut self,
		operation: &str,
		request: Uuid,
		fingerprint: String,
		result: Value,
	) -> aidash_application::Result<()> {
		put(
			&mut self.access.tx,
			"marketplace_requests",
			&key(&(
				self.access.identity.tenant.as_str(),
				self.access.identity.subject.as_str(),
				operation,
				request,
			)),
			&Replay {
				fingerprint,
				result,
			},
		)
		.await
		.map_err(Into::into)
	}
}
async fn persist_revision(
	tx: &mut Transaction<'_, Postgres>,
	install: &Installation,
	revision: &Revision,
) -> Result<()> {
	let entry = &revision.entry;
	let revision_number = revision.revision;
	if revision_number == 1 {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("marketplace_installations"))
				.columns(["key", "document", "tenant", "package_key"].map(Alias::new))
				.from_subquery({
					let mut values = Query::select();
					for i in 1..=4 {
						values.expr(Expr::cust(format!("${i}")));
					}
					values.to_owned()
				})
				.to_string(PostgresQueryBuilder),
		)
		.bind(&install.id)
		.bind(json!(install))
		.bind(&install.tenant)
		.bind(&install.package_key)
		.execute(&mut **tx)
		.await?;
	} else {
		put(tx, "marketplace_installations", &install.id, &install).await?;
	}
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("registry"))
			.columns(["id", "version", "kind", "metadata"].map(Alias::new))
			.from_subquery({
				let mut values = Query::select();
				for i in 1..=4 {
					values.expr(Expr::cust(format!("${i}")));
				}
				values.to_owned()
			})
			.to_string(PostgresQueryBuilder),
	)
	.bind(&entry.id)
	.bind(&entry.version)
	.bind(&entry.kind)
	.bind(json!(entry))
	.execute(&mut **tx)
	.await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("marketplace_revisions"))
			.columns(
				[
					"key",
					"document",
					"installation",
					"revision",
					"entry_id",
					"entry_version",
				]
				.map(Alias::new),
			)
			.from_subquery({
				let mut values = Query::select();
				for i in 1..=6 {
					values.expr(Expr::cust(format!("${i}")));
				}

				values.to_owned()
			})
			.to_string(PostgresQueryBuilder),
	)
	.bind(key(&(&install.id, revision_number)))
	.bind(json!(revision))
	.bind(&install.id)
	.bind(revision_number)
	.bind(&entry.id)
	.bind(&entry.version)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
async fn documents(tx: &mut Transaction<'_, Postgres>, original: &Entry) -> Result<Option<Value>> {
	let documents = {
		let query_bind_1 = &original.id;
		let query_bind_2 = &original.version;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("documents"))
				.from(Alias::new("agent_knowledge"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(agent_id=? AND agent_version=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **tx)
		.await?
	};
	Ok(documents)
}
async fn insert_documents(
	tx: &mut Transaction<'_, Postgres>,
	entry: &Entry,
	documents: Value,
) -> Result<()> {
	{
		let query_bind_1 = &entry.id;
		let query_bind_2 = &entry.version;
		let query_bind_3 = documents;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("agent_knowledge"))
				.columns(["agent_id", "agent_version", "documents"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	};
	Ok(())
}
pub(crate) async fn approved(
	tx: &mut Transaction<'_, Postgres>,
	tenant: &str,
	entry: &EntityRef,
) -> Result<bool> {
	let approved: Option<bool> = {
		let query_bind_1 = tenant;
		let query_bind_2 = &entry.id;
		let query_bind_3 = &entry.version;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("enabled"))
				.from(Alias::new("authorization_catalog"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant=? AND entry_id=? AND entry_version=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **tx)
		.await?
	};
	Ok(approved == Some(true))
}

pub(crate) async fn revision(
	tx: &mut Transaction<'_, Postgres>,
	id: &str,
	revision: i64,
) -> Result<Revision> {
	get(tx, "marketplace_revisions", &key(&(id, revision)))
		.await?
		.ok_or(Error::Forbidden)
}
pub(crate) async fn provenance(
	tx: &mut Transaction<'_, Postgres>,
	entry: &Entry,
	tenant: &str,
) -> Result<BTreeSet<ConsentEdge>> {
	let mut edges: BTreeSet<ConsentEdge> =
		get(tx, "marketplace_provenance", &key(&reference(entry)))
			.await?
			.unwrap_or_default();
	let copies: BTreeSet<ConsentEdge> = get(
		tx,
		"marketplace_provenance",
		&format!("content-{}", key(&(tenant, content(entry)))),
	)
	.await?
	.unwrap_or_default();
	edges.extend(copies);
	Ok(edges)
}
pub(crate) struct NativeProvenance<'a, 'connection> {
	pub(crate) tx: &'a mut Transaction<'connection, Postgres>,
}
#[async_trait]
impl aidash_application::ports::marketplace::ProvenanceScope for NativeProvenance<'_, '_> {
	async fn raw_definition(&mut self, reference: &EntityRef) -> aidash_application::Result<Entry> {
		super::definitions::raw(self.tx, reference)
			.await
			.map_err(Into::into)
	}
	async fn provenance(
		&mut self,
		entry: &Entry,
		tenant: &str,
	) -> aidash_application::Result<BTreeSet<ConsentEdge>> {
		provenance(self.tx, entry, tenant).await.map_err(Into::into)
	}
	async fn save_provenance(
		&mut self,
		entry: &Entry,
		edges: &BTreeSet<ConsentEdge>,
	) -> aidash_application::Result<()> {
		put(
			self.tx,
			"marketplace_provenance",
			&key(&reference(entry)),
			edges,
		)
		.await
		.map_err(Into::into)
	}
}
