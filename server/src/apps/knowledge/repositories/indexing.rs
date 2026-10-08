//! Original skip-locked selections, authority fences, and cleanup transactions.
use crate::apps::knowledge::{
	models::{SemanticCollection, SemanticPoint},
	services::core::{
		EmbeddingConfig, Entry, Source,
		service::{self, Lease},
	},
};
use crate::{Error, store::Store, transactions::gate::ReadLease};
use aidash_application::{Result, ports::semantic::*};
use aidash_domain::semantic::indexing::{IndexingEntry, IndexingPlan};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, ColumnRef, Expr, LockBehavior, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeIndexing {
	pub store: Store,
}
struct Visibility {
	_lease: ReadLease,
}
#[async_trait]
impl SemanticVisibility for Visibility {}
struct Session {
	store: Store,
	lease: Lease<'static>,
	entry: Option<Entry>,
}
struct Cleanup {
	transaction: Box<dyn TransactionExecutor>,
}

fn snapshot(entry: &Entry) -> IndexingEntry {
	IndexingEntry {
		id: entry.id,
		workspace_id: entry.workspace_id,
		source: entry.source.clone(),
		revision: entry.revision,
		point_id: entry.point_id,
		state: entry.state.clone(),
		attempts: entry.attempts,
	}
}
#[async_trait]
impl SemanticIndexingRepository for NativeIndexing {
	async fn maintenance(&self) -> Result<()> {
		super::receiver_caches::sweep(&self.store).await?;
		if let Some(directory) = std::env::var_os("AIDASH_MEMORY_RECOVERY_DIR") {
			crate::semantic::services::memory_recovery::prune_expired(
				&self.store,
				std::path::Path::new(&directory),
			)
			.await?;
		}
		super::retention::sweep(&self.store).await?;
		super::purge::sweep(&self.store).await?;
		super::engine_jobs::sweep(&self.store).await?;
		Ok(())
	}
	async fn begin_visibility(&self) -> Result<Box<dyn SemanticVisibility>> {
		Ok(Box::new(Visibility {
			_lease: ReadLease::begin(&self.store).await?,
		}))
	}
	async fn due(&self) -> Result<Vec<Uuid>> {
		let store = &self.store;
		let ids: Vec<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.expr(SimpleExpr::from(Expr::col(Alias::new("id"))))
				.from(Alias::new("semantic_entries"))
				.and_where(Expr::cust(
					"NOT deleted AND next_attempt <= CLOCK_TIMESTAMP()",
				))
				.order_by_expr(
					SimpleExpr::from(Expr::col(Alias::new("next_attempt"))),
					Order::Asc,
				)
				.order_by_expr(SimpleExpr::from(Expr::col(Alias::new("id"))), Order::Asc)
				.limit(32)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&store.pool)
		.await?;
		Ok(ids)
	}
	async fn initial(&self, id: Uuid) -> Result<Option<(Uuid, Value)>> {
		let store = &self.store;
		Ok(crate::database::native::query_as::<(Uuid, Value)>(
			&Query::select()
				.expr(SimpleExpr::from(Expr::col(Alias::new("workspace_id"))))
				.expr(SimpleExpr::from(Expr::col(Alias::new("authority"))))
				.from(Alias::new("semantic_entries"))
				.and_where(Expr::cust("id = $1 AND NOT deleted"))
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["workspace_id", "authority"])
		.bind(id)
		.fetch_optional(&store.pool)
		.await?)
	}
	async fn restore(&self, authority: &Value) -> Result<Box<dyn SemanticIndexingSession>> {
		Ok(Box::new(Session {
			store: self.store.clone(),
			lease: Lease::restore(&self.store, authority.clone()).await?,
			entry: None,
		}))
	}
	async fn revoke(&self, workspace: Uuid, id: Uuid, authority: &Value) -> Result<()> {
		let store = &self.store;
		let result: crate::Result<()> = async {
			let mut tx = crate::database::native::begin(&store.pool).await?;
			service::index(&mut tx, workspace, false).await?;
			let entry: Option<Entry> = {
				let query_bind_1 = id;
				let query_bind_2 = authority;
				crate::database::native::query_as(
					&Query::update()
						.table(Alias::new("semantic_entries"))
						.value_expr(Alias::new("state"), Expr::cust("'REVOKED'"))
						.value_expr(
							Alias::new("last_error"),
							Expr::cust("'indexing credential or policy revoked'"),
						)
						.value_expr(
							Alias::new("next_attempt"),
							Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ? AND authority = ? AND NOT deleted)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.returning_all()
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut *tx)
				.await?
			};
			if let Some(entry) = entry {
				{
					let query_bind_1 = id;
					crate::database::native::query(
						&Query::update()
							.table(Alias::new("semantic_points"))
							.value_expr(Alias::new("retired"), Expr::cust("TRUE"))
							.value_expr(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(entry_id = ? AND NOT retired)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.execute(&mut *tx)
					.await?
				};
				service::history(
					&mut tx,
					workspace,
					Some(id),
					entry.revision,
					"REVOKED",
					"indexing authority unavailable",
				)
				.await?;
			}
			tx.commit().await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn cleanup_due(&self) -> Result<CleanupBatch> {
		let database = self.store.database();
		let points = SemanticPoint::cleanup_due(&database).await?;
		let collections = SemanticCollection::cleanup_due(&database).await?;
		Ok(CleanupBatch {
			points,
			collections,
		})
	}
	async fn begin_cleanup(&self) -> Result<Box<dyn SemanticCleanupSession>> {
		Ok(Box::new(Cleanup {
			transaction: self.store.database().begin().await.map_err(Error::from)?,
		}))
	}
}
#[async_trait]
impl SemanticIndexingSession for Session {
	fn durable(&mut self) {
		self.lease.durable();
	}
	async fn workspace_write(&mut self, workspace: Uuid) -> Result<()> {
		self.lease
			.workspace(workspace, "semantic.write")
			.await
			.map_err(Into::into)
	}
	async fn plan(&mut self, workspace: Uuid) -> Result<IndexingPlan> {
		// Fence maintenance before source/settings reads: its workspace writer
		// would otherwise block the independent reservation's foreign-key lock
		// while waiting for those reads to finish.
		super::native_memory::lock_workspace(&mut self.lease, workspace, false).await?;
		let index = service::index(self.lease.tx(), workspace, false).await?;
		Ok(IndexingPlan {
			collection: index.collection,
			tenant: index.tenant,
			revision: index.revision,
			spec: index.spec,
		})
	}
	async fn lock_entry(&mut self, id: Uuid) -> Result<Option<IndexingEntry>> {
		let lease = &mut self.lease;
		let entry: Option<Entry> = {
			let query_bind_1 = id;
			crate::database::native::query_as::<Entry>(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
					.from(Alias::new("semantic_entries"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND NOT deleted AND next_attempt <= CLOCK_TIMESTAMP())".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Update)
					.lock_behavior(LockBehavior::SkipLocked)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **lease.tx())
			.await?
		};
		let output = entry.as_ref().map(snapshot);
		self.entry = entry;
		Ok(output)
	}
	async fn authority(&mut self, id: Uuid) -> Result<Value> {
		let lease = &mut self.lease;
		Ok({
			let query_bind_1 = id;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(Alias::new("authority"))))
					.from(Alias::new("semantic_entries"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut **lease.tx())
			.await?
		})
	}
	async fn permits(&mut self, action: &str) -> Result<bool> {
		let entry = self
			.entry
			.as_ref()
			.ok_or_else(|| Error::Conflict("indexing entry was not locked".into()))?;
		self.lease.permits(entry, action).await.map_err(Into::into)
	}
	async fn source(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		self.lease
			.source(workspace, source)
			.await
			.map_err(Into::into)
	}
	async fn set_revoked(&mut self, id: Uuid, reason: &str) -> Result<()> {
		let lease = &mut self.lease;
		{
			let query_bind_1 = id;
			let query_bind_2 = reason;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("semantic_entries"))
					.value_expr(Alias::new("state"), Expr::cust("'REVOKED'"))
					.value_expr(
						Alias::new("last_error"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("next_attempt"),
						Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?
		};
		Ok(())
	}
	async fn retire_points(&mut self, id: Uuid) -> Result<()> {
		let lease = &mut self.lease;
		{
			let query_bind_1 = id;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("semantic_points"))
					.value_expr(Alias::new("retired"), Expr::cust("TRUE"))
					.value_expr(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(entry_id = ? AND NOT retired)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?
		};
		Ok(())
	}
	async fn defer(&mut self, id: Uuid) -> Result<()> {
		let lease = &mut self.lease;
		{
			let query_bind_1 = id;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("semantic_entries"))
					.value_expr(
						Alias::new("next_attempt"),
						Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?
		};
		Ok(())
	}
	async fn record_digest(&mut self, point: Uuid, digest: &str) -> Result<()> {
		let lease = &mut self.lease;
		let digest = digest.to_owned();
		{
			let query_bind_1 = point;
			let query_bind_2 = digest;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("semantic_points"))
					.value_expr(
						Alias::new("content_digest"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND NOT retired)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?
		};
		Ok(())
	}
	async fn ready(&mut self, id: Uuid) -> Result<()> {
		let lease = &mut self.lease;
		{
			let query_bind_1 = id;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("semantic_entries"))
					.value_expr(Alias::new("state"), Expr::cust("'READY'"))
					.value_expr(Alias::new("attempts"), Expr::cust("0"))
					.value_expr(Alias::new("last_error"), Expr::cust("NULL"))
					.value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()"))
					.value_expr(
						Alias::new("next_attempt"),
						Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 SECONDS'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?
		};
		Ok(())
	}
	async fn failed(&mut self, id: Uuid, attempts: i32, delay: f64) -> Result<()> {
		let lease = &mut self.lease;
		{
			let query_bind_1 = id;
			let query_bind_2 = attempts;
			let query_bind_3 = delay;
			crate::database::native::query(
				&Query::update()
					.table(Alias::new("semantic_entries"))
					.value_expr(Alias::new("state"), Expr::cust("'ERROR'"))
					.value_expr(
						Alias::new("attempts"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						Alias::new("last_error"),
						Expr::cust(
							"'embedding or vector backend unavailable, invalid, or source too large'",
						),
					)
					.value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()"))
					.value_expr(
						Alias::new("next_attempt"),
						SimpleExpr::CustomWithExpr(
							"(CLOCK_TIMESTAMP() + MAKE_INTERVAL(secs => ?))".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?
		};
		Ok(())
	}
	async fn point(&mut self, id: Uuid) -> Result<Option<(Option<String>, bool)>> {
		let lease = &mut self.lease;
		let entry_point = id;
		Ok({
			let query_bind_1 = entry_point;
			crate::database::native::query_as(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(Alias::new("content_digest"))))
					.expr(SimpleExpr::from(Expr::col(Alias::new("retired"))))
					.from(Alias::new("semantic_points"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["content_digest", "retired"])
			.fetch_optional(&mut **lease.tx())
			.await?
		})
	}
	async fn rotate(&mut self, id: Uuid, point: Uuid) -> Result<IndexingEntry> {
		let lease = &mut self.lease;
		let entry: Entry = {
			let query_bind_1 = id;
			let query_bind_2 = point;
			crate::database::native::query_as(
				&Query::update()
					.table(Alias::new("semantic_entries"))
					.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
					.value_expr(
						Alias::new("point_id"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(Alias::new("state"), Expr::cust("'PENDING'"))
					.value_expr(Alias::new("attempts"), Expr::cust("0"))
					.value_expr(Alias::new("last_error"), Expr::cust("NULL"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **lease.tx())
			.await?
		};
		let output = snapshot(&entry);
		self.entry = Some(entry);
		Ok(output)
	}
	async fn schedule_point(&mut self, collection: &str) -> Result<()> {
		let entry = self
			.entry
			.as_ref()
			.ok_or_else(|| Error::Conflict("indexing entry was not locked".into()))?;
		service::schedule_point(self.lease.tx(), entry, collection)
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
	async fn embed(
		&mut self,
		workspace: Uuid,
		config: &EmbeddingConfig,
		text: &str,
		entry: Uuid,
	) -> Result<Vec<f32>> {
		if let Some(row) = &self.entry
			&& matches!(
				serde_json::from_value::<Source>(row.source.clone())?,
				Source::Unit { .. }
			) {
			use aidash_application::ports::memory::Allowance;
			let provider = serde_json::from_value(row.metadata["provider"].clone())?;
			let bank = serde_json::from_value(row.metadata["bank"].clone())?;
			let policy = crate::semantic::native_memory::policy(&mut self.lease, &provider).await?;
			let pinned: EmbeddingConfig = serde_json::from_value(
				crate::semantic::native_memory::definition(
					&mut self.lease,
					&policy.embedding,
					"embedding",
				)
				.await?
				.config,
			)?;
			if serde_json::to_value(config)? != serde_json::to_value(&pinned)? {
				return Err(Error::Conflict(
					"memory embedding generation requires explicit reindexing".into(),
				)
				.into());
			}
			let key = serde_json::to_string(
				&serde_json::json!({"kind":"memory-index","unit":entry,"unit_revision":row.metadata["unit_revision"],"point":row.point_id,"index_revision":row.index_revision,"provider":provider,"text_digest":aidash_domain::semantic::indexing::content_digest(text)}),
			)?;
			let origin = super::unit_origins::load(&mut self.lease, entry)
				.await?
				.ok_or(Error::Forbidden)?;
			if Some(origin.revision) != row.metadata["unit_revision"].as_i64() {
				return Err(Error::Forbidden.into());
			}
			let mut models =
				crate::semantic::services::memory_models::Models::resolve_index_origin(
					&self.store,
					&mut self.lease,
					provider,
					bank,
					policy.clone(),
					crate::semantic::native_memory::request_id(entry, &key)?,
					aidash_domain::semantic::indexing::content_digest(&key),
					&origin.runs,
				)
				.await?;
			models.embedding_origin = Some(crate::generation::embedding::Origin::Index(entry));
			let produced = models
				.embed(
					&policy.embedding,
					text,
					Allowance {
						calls: policy.bounds.max_model_calls,
						tokens: policy.bounds.max_model_tokens,
						cost_micros: policy.bounds.max_cost_micros,
					},
				)
				.await?;
			return Ok(produced.output);
		}

		service::embed(
			&self.store,
			&mut self.lease,
			workspace,
			config,
			text,
			crate::generation::embedding::Origin::Index(entry),
		)
		.await
		.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<bool> {
		self.lease
			.finish(result.map_err(Error::from))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl SemanticCleanupSession for Cleanup {
	async fn lock_point(
		&mut self,
		id: Uuid,
	) -> Result<Option<(String, aidash_domain::semantic::VectorConfig)>> {
		SemanticPoint::lock_cleanup(self.transaction.as_mut(), id)
			.await
			.map_err(Into::into)
	}
	async fn lock_collection(
		&mut self,
		collection: &str,
	) -> Result<Option<aidash_domain::semantic::VectorConfig>> {
		SemanticCollection::lock_cleanup(self.transaction.as_mut(), collection)
			.await
			.map_err(Into::into)
	}
	async fn record_point(&mut self, id: Uuid, failure: Option<&str>) -> Result<()> {
		SemanticPoint::record_cleanup(self.transaction.as_mut(), id, failure)
			.await
			.map_err(Into::into)
	}
	async fn record_collection(&mut self, collection: &str, failure: Option<&str>) -> Result<()> {
		SemanticCollection::record_cleanup(self.transaction.as_mut(), collection, failure)
			.await
			.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.transaction.commit().await.map_err(Error::from)?;
		Ok(())
	}
}
