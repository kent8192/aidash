//! Entry mutations preserve their original Query statements and leased connection.
use crate::apps::knowledge::{
	models::{SemanticCollection, SemanticEntry, SemanticHistory, SemanticIndexe, SemanticPoint},
	services::core::{
		self as native,
		service::{self, Lease},
	},
};
use crate::apps::{
	identity::models::AuthorizationWorkspace, workspaces::models::Workspace as WorkspaceRecord,
};
use crate::{Error, Result as NativeResult, store::Store};
use aidash_application::{Result, ports::semantic::mutations::*};
use aidash_domain::semantic::{
	Source,
	indexing::IndexingSpec,
	mutations::{Entry, History, Index, Put},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, LockType, OnConflict, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeConfiguration {
	pub store: Store,
}
struct Configuration {
	transaction: Box<dyn TransactionExecutor>,
}
pub(crate) struct Entries<'a, 'scope> {
	pub lease: &'a mut Lease<'scope>,
}

#[async_trait]
impl SemanticConfigurationRepository for NativeConfiguration {
	fn validate(&self, spec: &IndexingSpec) -> Result<()> {
		native::IndexSpec::from(spec.clone())
			.validate()
			.map_err(Into::into)
	}
	async fn begin(&self) -> Result<Box<dyn SemanticConfigurationSession>> {
		Ok(Box::new(Configuration {
			transaction: self.store.database().begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl SemanticConfigurationSession for Configuration {
	async fn lock_workspace(&mut self, workspace: Uuid) -> Result<()> {
		WorkspaceRecord::lock_existing(self.transaction.as_mut(), workspace)
			.await
			.map_err(Into::into)
	}
	async fn tenant(&mut self, workspace: Uuid) -> Result<String> {
		Ok(
			AuthorizationWorkspace::tenant_in(self.transaction.as_mut(), workspace)
				.await?
				.unwrap_or_default(),
		)
	}
	async fn index(&mut self, workspace: Uuid) -> Result<Option<Index>> {
		Ok(
			SemanticIndexe::locked(self.transaction.as_mut(), workspace, true)
				.await?
				.map(Into::into),
		)
	}
	async fn counts(&mut self, workspace: Uuid) -> Result<(i64, usize)> {
		SemanticEntry::configuration_counts(self.transaction.as_mut(), workspace)
			.await
			.map_err(Into::into)
	}
	async fn replace(
		&mut self,
		workspace: Uuid,
		tenant: &str,
		revision: i64,
		spec: Value,
		collection: &str,
	) -> Result<Index> {
		// The Workspace lock serializes index generations with bank policy pins.
		// Include empty banks: their next projection and recall must remain usable.
		let proposed: IndexingSpec = serde_json::from_value(spec.clone())?;
		let mut after = Uuid::nil();
		let mut checked = std::collections::BTreeSet::new();
		loop {
			let rows = crate::database::native::query(
				&Query::select()
					.columns([
						("s", "bank_id"),
						("s", "provider_id"),
						("s", "provider_version"),
					])
					.from_as(Alias::new("memory_bank_settings"), Alias::new("s"))
					.inner_join(
						Alias::new("memory_banks"),
						Expr::col(("s", "bank_id")).equals(("memory_banks", "id")),
					)
					.and_where(
						Expr::col(("memory_banks", "workspace_id")).eq(Expr::value(workspace)),
					)
					.and_where(Expr::col(("s", "bank_id")).gt(Expr::value(after)))
					.order_by(("s", "bank_id"), Order::Asc)
					.limit(256)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(self.transaction.as_mut())
			.await?;
			if rows.is_empty() {
				break;
			}
			for row in rows {
				if !proposed.enabled {
					return Err(aidash_application::Error::Conflict(
						"Workspace index must remain enabled for pinned memory banks".into(),
					));
				}
				after = row.try_get("bank_id")?;
				let reference = (
					row.try_get::<String>("provider_id")?,
					row.try_get::<String>("provider_version")?,
				);
				if !checked.insert(reference.clone()) {
					continue;
				}
				let provider = crate::apps::registry::models::Definition::read_in(
					self.transaction.as_mut(),
					&reference.0,
					&reference.1,
				)
				.await?;
				let config: aidash_domain::memory::ProviderConfig =
					serde_json::from_value(provider.config)?;
				let role = &config.policy.embedding;
				let embedding = crate::apps::registry::models::Definition::read_in(
					self.transaction.as_mut(),
					&role.id,
					&role.version,
				)
				.await?;
				let embedding: aidash_domain::semantic::EmbeddingConfig =
					serde_json::from_value(embedding.config)?;
				if proposed.embedding != embedding {
					return Err(aidash_application::Error::Conflict(
						"Workspace index embedding differs from a pinned memory bank policy".into(),
					));
				}
			}
		}
		Ok(SemanticIndexe::replace_generation(
			self.transaction.as_mut(),
			workspace,
			tenant,
			revision,
			spec,
			collection,
		)
		.await?
		.into())
	}
	async fn retire_collections(&mut self, workspace: Uuid) -> Result<()> {
		SemanticCollection::retire_workspace(self.transaction.as_mut(), workspace)
			.await
			.map_err(Into::into)
	}
	async fn record_collection(
		&mut self,
		workspace: Uuid,
		collection: &str,
		vector: Value,
	) -> Result<()> {
		SemanticCollection::record_generation(
			self.transaction.as_mut(),
			collection,
			workspace,
			vector,
		)
		.await
		.map_err(Into::into)
	}
	async fn active(&mut self, workspace: Uuid) -> Result<Vec<Uuid>> {
		Ok(
			SemanticEntry::lock_active(self.transaction.as_mut(), workspace)
				.await?
				.into_iter()
				.map(|entry| entry.id)
				.collect(),
		)
	}
	async fn reconfigure(&mut self, id: Uuid, revision: i64) -> Result<Entry> {
		Ok(
			SemanticEntry::reconfigure(self.transaction.as_mut(), id, revision)
				.await?
				.into(),
		)
	}
	async fn schedule(&mut self, entry: &Entry, collection: &str) -> Result<()> {
		SemanticPoint::schedule(
			self.transaction.as_mut(),
			&native::Entry::from(entry.clone()),
			collection,
		)
		.await
		.map_err(Into::into)
	}
	async fn history(&mut self, workspace: Uuid, revision: i64) -> Result<()> {
		SemanticHistory::append(
			self.transaction.as_mut(),
			workspace,
			None,
			revision,
			"CONFIGURED",
			"new immutable index generation",
		)
		.await
		.map_err(Into::into)
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.transaction
			.commit()
			.await
			.map_err(Error::from)
			.map_err(Into::into)
	}
}
#[async_trait]
impl SemanticEntriesSession for Entries<'_, '_> {
	async fn workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		self.lease
			.workspace(workspace, action)
			.await
			.map_err(Into::into)
	}
	async fn index(&mut self, workspace: Uuid, exclusive: bool) -> Result<Index> {
		Ok(service::index(self.lease.tx(), workspace, exclusive)
			.await?
			.into())
	}
	fn saved(&mut self) -> Result<Value> {
		self.lease.saved().map_err(Into::into)
	}
	async fn permits(&mut self, entry: &Entry, action: &str) -> Result<bool> {
		self.lease
			.permits(&native::Entry::from(entry.clone()), action)
			.await
			.map_err(Into::into)
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.lease
			.source(workspace, source)
			.await
			.map_err(Into::into)
	}
	async fn schedule(&mut self, entry: &Entry, collection: &str) -> Result<()> {
		service::schedule_point(
			self.lease.tx(),
			&native::Entry::from(entry.clone()),
			collection,
		)
		.await
		.map_err(Into::into)
	}
	async fn history(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		revision: i64,
		state: &str,
		detail: &str,
	) -> Result<()> {
		service::history(
			self.lease.tx(),
			workspace,
			Some(id),
			revision,
			state,
			detail,
		)
		.await
		.map_err(Into::into)
	}

	async fn by_key(&mut self, workspace: Uuid, key: &str) -> Result<Option<Entry>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Option<native::Entry>> = async {
			let value: Option<native::Entry> = {
				let query_bind_1 = workspace;
				let query_bind_2 = key;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND key = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result
			.map(|value| value.map(Into::into))
			.map_err(Into::into)
	}

	async fn count(&mut self, workspace: Uuid) -> Result<i64> {
		let lease = &mut *self.lease;

		let result: NativeResult<i64> = async {
			let value: i64 = {
				let query_bind_1 = workspace;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(Expr::cust("COUNT(*)"))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND NOT deleted)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result.map_err(Into::into)
	}

	async fn put(&mut self, entry: Entry, authority: Value) -> Result<Entry> {
		let lease = &mut *self.lease;
		let workspace = entry.workspace_id;
		let saved = authority;
		let result: NativeResult<native::Entry> = async {
			let value: native::Entry = {
				let query_bind_1 = entry.id;
				let query_bind_2 = workspace;
				let query_bind_3 = entry.key;
				let query_bind_4 = entry.source;
				let query_bind_5 = entry.agent;
				let query_bind_6 = entry.metadata;
				let query_bind_7 = entry.revision;
				let query_bind_8 = entry.point_id;
				let query_bind_9 = entry.index_revision;
				let query_bind_10 = entry.created_by;
				let query_bind_11 = saved;
				crate::database::native::query_as(
					&Query::insert()
						.into_table(Alias::new("semantic_entries"))
						.columns([
							Alias::new("id"),
							Alias::new("workspace_id"),
							Alias::new("key"),
							Alias::new("source"),
							Alias::new("agent"),
							Alias::new("metadata"),
							Alias::new("revision"),
							Alias::new("point_id"),
							Alias::new("index_revision"),
							Alias::new("deleted"),
							Alias::new("state"),
							Alias::new("created_by"),
							Alias::new("authority"),
						])
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
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_7.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_8.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_9.to_owned()).into()],
								))
								.expr(Expr::cust("FALSE"))
								.expr(Expr::cust("'PENDING'"))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_10.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_11.to_owned()).into()],
								))
								.to_owned(),
						)
						.on_conflict(OnConflict::columns([Alias::new("id")]).update_columns([
							Alias::new("source"),
							Alias::new("agent"),
							Alias::new("metadata"),
							Alias::new("revision"),
							Alias::new("point_id"),
							Alias::new("index_revision"),
							Alias::new("state"),
							Alias::new("attempts"),
							Alias::new("last_error"),
							Alias::new("authority"),
							Alias::new("next_attempt"),
							Alias::new("updated_at"),
						]))
						.returning_all()
						.to_string(PostgresQueryBuilder),
				)
				.columns(&[
					"id",
					"workspace_id",
					"key",
					"source",
					"agent",
					"metadata",
					"revision",
					"point_id",
					"index_revision",
					"deleted",
					"state",
					"created_by",
					"authority",
				])
				.fetch_one(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result.map(|value| value.into()).map_err(Into::into)
	}

	async fn list(&mut self, workspace: Uuid) -> Result<Vec<Entry>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Vec<native::Entry>> = async {
			let value: Vec<native::Entry> = {
				let query_bind_1 = workspace;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND NOT deleted)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.order_by_expr(SimpleExpr::from(Expr::col(Alias::new("id"))), Order::Asc)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result
			.map(|value| value.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}

	async fn lock_entry(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Entry>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Option<native::Entry>> = async {
			let value: Option<native::Entry> = {
				let query_bind_1 = workspace;
				let query_bind_2 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(workspace_id = ? AND id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result
			.map(|value| value.map(Into::into))
			.map_err(Into::into)
	}

	async fn reindex_replay(&mut self, id: Uuid, revision: i64) -> Result<bool> {
		let lease = &mut *self.lease;

		let result: NativeResult<bool>=async { let value: bool={ let query_bind_1 = id; let query_bind_2 = revision; crate::database::native::query_scalar(&Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM semantic_history WHERE entry_id = ? AND revision = ? AND state = 'PENDING' AND detail = 'reindex requested'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()])).to_string(PostgresQueryBuilder)).scalar_one(&mut **lease.tx()).await? }; Ok(value) }.await;
		result.map_err(Into::into)
	}

	async fn change(
		&mut self,
		workspace: Uuid,
		id: Uuid,
		point: Uuid,
		index_revision: i64,
		delete: bool,
		authority: Value,
	) -> Result<Entry> {
		let lease = &mut *self.lease;
		let saved = authority;
		let result: NativeResult<native::Entry>=async { let value: native::Entry={ let query_bind_1 = id; let query_bind_2 = workspace; let query_bind_3 = point; let query_bind_4 = index_revision; let query_bind_5 = delete; let query_bind_6 = if delete {"DELETED"} else {"PENDING"}; let query_bind_7 = saved; crate::database::native::query_as(&Query::update().table(Alias::new("semantic_entries")).value_expr(Alias::new("revision"), Expr::cust("revision + 1")).value_expr(Alias::new("point_id"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(Alias::new("index_revision"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.to_owned()).into()])).value_expr(Alias::new("deleted"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(Alias::new("state"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).value_expr(Alias::new("source"), SimpleExpr::CustomWithExpr("(CASE WHEN ? THEN JSONB_BUILD_OBJECT('kind', 'memory', 'text', '') ELSE source END)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(Alias::new("authority"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_7.to_owned()).into()])).value_expr(Alias::new("attempts"), Expr::cust("0")).value_expr(Alias::new("last_error"), Expr::cust("NULL")).value_expr(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()")).value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()")).and_where(SimpleExpr::CustomWithExpr("(id = ? AND workspace_id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()])).returning_all().to_string(PostgresQueryBuilder)).fetch_one(&mut **lease.tx()).await? }; Ok(value) }.await;
		result.map(|value| value.into()).map_err(Into::into)
	}

	async fn history_page(&mut self, workspace: Uuid, cursor: i64) -> Result<Vec<History>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Vec<native::History>> = async {
			let value: Vec<native::History> = {
				let query_bind_1 = workspace;
				let query_bind_2 = cursor;
				crate::database::native::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("semantic_history"))
						.and_where(
							Expr::col(Alias::new("workspace_id"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("sequence"))
								.lt(Expr::value(query_bind_2.to_owned())),
						)
						.order_by(Alias::new("sequence"), Order::Desc)
						.limit(200)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result
			.map(|value| value.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}

	async fn history_entry(&mut self, id: Uuid) -> Result<Entry> {
		let lease = &mut *self.lease;

		let result: NativeResult<native::Entry> = async {
			let value: native::Entry = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **lease.tx())
				.await?
			};
			Ok(value)
		}
		.await;
		result.map(|value| value.into()).map_err(Into::into)
	}
}

impl From<native::Index> for Index {
	fn from(value: native::Index) -> Self {
		Self {
			workspace_id: value.workspace_id,
			tenant: value.tenant,
			revision: value.revision,
			spec: value.spec,
			collection: value.collection,
			updated_at: value.updated_at,
		}
	}
}
impl From<Index> for native::Index {
	fn from(value: Index) -> Self {
		Self {
			workspace_id: value.workspace_id,
			tenant: value.tenant,
			revision: value.revision,
			spec: value.spec,
			collection: value.collection,
			updated_at: value.updated_at,
		}
	}
}
impl From<native::Entry> for Entry {
	fn from(value: native::Entry) -> Self {
		Self {
			id: value.id,
			workspace_id: value.workspace_id,
			key: value.key,
			source: value.source,
			agent: value.agent,
			metadata: value.metadata,
			revision: value.revision,
			point_id: value.point_id,
			index_revision: value.index_revision,
			deleted: value.deleted,
			state: value.state,
			attempts: value.attempts,
			last_error: value.last_error,
			created_by: value.created_by,
			updated_at: value.updated_at,
		}
	}
}
impl From<Entry> for native::Entry {
	fn from(value: Entry) -> Self {
		Self {
			id: value.id,
			workspace_id: value.workspace_id,
			key: value.key,
			source: value.source,
			agent: value.agent,
			metadata: value.metadata,
			revision: value.revision,
			point_id: value.point_id,
			index_revision: value.index_revision,
			deleted: value.deleted,
			state: value.state,
			attempts: value.attempts,
			last_error: value.last_error,
			created_by: value.created_by,
			updated_at: value.updated_at,
		}
	}
}
impl From<native::History> for History {
	fn from(value: native::History) -> Self {
		Self {
			sequence: value.sequence,
			workspace_id: value.workspace_id,
			entry_id: value.entry_id,
			revision: value.revision,
			state: value.state,
			detail: value.detail,
			created_at: value.created_at,
		}
	}
}
impl From<History> for native::History {
	fn from(value: History) -> Self {
		Self {
			sequence: value.sequence,
			workspace_id: value.workspace_id,
			entry_id: value.entry_id,
			revision: value.revision,
			state: value.state,
			detail: value.detail,
			created_at: value.created_at,
		}
	}
}
impl From<native::PutEntry> for Put {
	fn from(value: native::PutEntry) -> Self {
		Self {
			key: value.key,
			expected_revision: value.expected_revision,
			source: value.source,
			agent: value.agent,
			metadata: value.metadata,
		}
	}
}
impl From<Put> for native::PutEntry {
	fn from(value: Put) -> Self {
		Self {
			key: value.key,
			expected_revision: value.expected_revision,
			source: value.source,
			agent: value.agent,
			metadata: value.metadata,
		}
	}
}
impl From<native::IndexSpec> for IndexingSpec {
	fn from(value: native::IndexSpec) -> Self {
		Self {
			embedding: value.embedding,
			vector: value.vector,
			enabled: value.enabled,
			auto_context: value.auto_context,
			max_sources: value.max_sources,
			max_results: value.max_results,
			max_result_tokens: value.max_result_tokens,
			max_input_bytes: value.max_input_bytes,
		}
	}
}
impl From<IndexingSpec> for native::IndexSpec {
	fn from(value: IndexingSpec) -> Self {
		Self {
			embedding: value.embedding,
			vector: value.vector,
			enabled: value.enabled,
			auto_context: value.auto_context,
			max_sources: value.max_sources,
			max_results: value.max_results,
			max_result_tokens: value.max_result_tokens,
			max_input_bytes: value.max_input_bytes,
		}
	}
}
