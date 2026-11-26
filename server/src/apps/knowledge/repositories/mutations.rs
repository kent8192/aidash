//! Entry mutations preserve their original Query statements and leased connection.
use crate::apps::knowledge::{
	models::{SemanticCollection, SemanticEntry, SemanticHistory, SemanticIndexe, SemanticPoint},
	services::core::{
		self as native,
		service::{self, Lease},
	},
};
use crate::apps::{
	identity::models::AuthorizationThread, workspaces::models::Thread as ThreadRecord,
};
use crate::authorization::identity::Actor;
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
	Alias, ColumnRef, Expr, ExprTrait as _, Func, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

/// Configuring a Thread's semantic index (#360).
pub(crate) const SEMANTIC_INDEX_MANAGE: &str = "semantic.index.manage";

/// Validate an index specification against this node's runtime composition.
pub(crate) fn validate_index(store: &Store, spec: &IndexingSpec) -> Result<()> {
	native::IndexSpec::from(spec.clone())
		.validate_with(&crate::bootstrap::registry_validation_for(store))?;
	// An address literal is refused when configured, as before any call; a
	// host name is checked again at each connection.
	if spec.embedding.provider_credential.is_none()
		&& store.keyless.destination(&spec.embedding.endpoint)?
			== aidash_domain::keyless::Destination::Forbidden
	{
		return Err(aidash_application::Error::Invalid(
			"a Keyless endpoint at a non-public address needs its origin in \
			 [keyless_endpoints] permitted_private_origins"
				.into(),
		));
	}
	Ok(())
}

/// Create the first index generation of a Thread that has none, in the
/// caller's transaction, and return its revision. A conversation creates its
/// new Thread and this index in one admission (#647); the caller has
/// already authorized `semantic.index.manage`.
pub(crate) async fn create_first_index(
	store: &Store,
	tx: &mut dyn TransactionExecutor,
	thread: Uuid,
	spec: &IndexingSpec,
	reason: &str,
) -> Result<i64> {
	validate_index(store, spec)?;
	ThreadRecord::lock_existing(tx, thread).await?;
	let tenant = AuthorizationThread::tenant_in(tx, thread)
		.await?
		.ok_or(aidash_application::Error::Forbidden)?;
	if SemanticIndexe::locked(tx, thread, true).await?.is_some() {
		return Err(aidash_application::Error::Conflict(
			"semantic index revision changed".into(),
		));
	}
	let collection = format!("aidash_{}", Uuid::new_v4().simple());
	let index = SemanticIndexe::replace_generation(
		tx,
		thread,
		&tenant,
		1,
		serde_json::to_value(spec)?,
		&collection,
	)
	.await?;
	SemanticCollection::record_generation(
		tx,
		&collection,
		thread,
		serde_json::to_value(&spec.vector)?,
	)
	.await?;
	SemanticHistory::append(tx, thread, None, index.revision, "CONFIGURED", reason).await?;
	Ok(index.revision)
}

pub(crate) struct NativeConfiguration {
	pub store: Store,
	pub actor: Actor,
	/// The Thread whose index is configured.
	pub thread: Uuid,
}
/// One configuration, decided and written under the caller's lease.
struct Configuration {
	lease: Option<Lease<'static>>,
}
impl Configuration {
	fn tx(&mut self) -> Result<&mut dyn TransactionExecutor> {
		Ok(self
			.lease
			.as_mut()
			.ok_or_else(|| Error::Conflict("semantic index configuration ended".into()))?
			.tx()
			.as_mut())
	}
}
pub(crate) struct Entries<'a, 'scope> {
	pub lease: &'a mut Lease<'scope>,
}

#[async_trait]
impl SemanticConfigurationRepository for NativeConfiguration {
	fn validate(&self, spec: &IndexingSpec) -> Result<()> {
		validate_index(&self.store, spec)
	}
	async fn begin(&self) -> Result<Box<dyn SemanticConfigurationSession>> {
		Ok(Box::new(Configuration {
			lease: Some(
				Lease::begin(&self.store, &self.actor, self.thread)
					.await
					.map_err(
						crate::authorization::remote::deployment_administrator::application_error,
					)?,
			),
		}))
	}
}
#[async_trait]
impl SemanticConfigurationSession for Configuration {
	async fn lock_thread(&mut self, thread: Uuid) -> Result<()> {
		ThreadRecord::lock_existing(self.tx()?, thread)
			.await
			.map_err(Into::into)
	}
	/// A Subject needs `semantic.index.manage` on the Thread; a denial is
	/// audited after this transaction rolls back.
	async fn authorize(&mut self, thread: Uuid) -> Result<()> {
		let mut lease = self
			.lease
			.take()
			.ok_or_else(|| Error::Conflict("semantic index configuration ended".into()))?;
		match lease.thread(thread, SEMANTIC_INDEX_MANAGE).await {
			Ok(()) => {
				self.lease = Some(lease);
				Ok(())
			}
			Err(error) => lease.finish(Err(error)).await.map_err(Into::into),
		}
	}
	async fn tenant(&mut self, thread: Uuid) -> Result<String> {
		AuthorizationThread::tenant_in(self.tx()?, thread)
			.await?
			.ok_or(aidash_application::Error::Forbidden)
	}
	async fn index(&mut self, thread: Uuid) -> Result<Option<Index>> {
		Ok(SemanticIndexe::locked(self.tx()?, thread, true)
			.await?
			.map(Into::into))
	}
	async fn counts(&mut self, thread: Uuid) -> Result<(i64, usize)> {
		SemanticEntry::configuration_counts(self.tx()?, thread)
			.await
			.map_err(Into::into)
	}
	async fn replace(
		&mut self,
		thread: Uuid,
		tenant: &str,
		revision: i64,
		spec: Value,
		collection: &str,
	) -> Result<Index> {
		// The Thread lock serializes index generations with bank policy pins.
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
						("memory_banks", "tenant"),
					])
					.from_as(Alias::new("memory_bank_settings"), Alias::new("s"))
					.inner_join(
						Alias::new("memory_banks"),
						Expr::col(("s", "bank_id")).equals(("memory_banks", "id")),
					)
					.and_where(Expr::col(("memory_banks", "thread_id")).eq(Expr::value(thread)))
					.and_where(Expr::col(("s", "bank_id")).gt(Expr::value(after)))
					.order_by(("s", "bank_id"), Order::Asc)
					.limit(256)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(self.tx()?)
			.await?;
			if rows.is_empty() {
				break;
			}
			for row in rows {
				after = row.try_get("bank_id")?;
				let reference = (
					row.try_get::<String>("tenant")?,
					row.try_get::<String>("provider_id")?,
					row.try_get::<String>("provider_version")?,
				);
				if !checked.insert(reference.clone()) {
					continue;
				}
				let provider = crate::apps::registry::models::Definition::read_of(
					self.tx()?,
					&reference.0,
					&reference.1,
					&reference.2,
				)
				.await?;
				let config: aidash_domain::memory::ProviderConfig =
					serde_json::from_value(provider.config)?;
				let Some(role) = &config.policy.embedding else {
					continue;
				};
				if !proposed.enabled {
					return Err(aidash_application::Error::Conflict(
						"Thread index must remain enabled for pinned memory banks".into(),
					));
				}
				let embedding = crate::apps::registry::models::Definition::read_of(
					self.tx()?,
					&reference.0,
					&role.id,
					&role.version,
				)
				.await?;
				let embedding: aidash_domain::semantic::EmbeddingConfig =
					serde_json::from_value(embedding.config)?;
				if proposed.embedding != embedding {
					return Err(aidash_application::Error::Conflict(
						"Thread index embedding differs from a pinned memory bank policy".into(),
					));
				}
			}
		}
		Ok(SemanticIndexe::replace_generation(
			self.tx()?,
			thread,
			tenant,
			revision,
			spec,
			collection,
		)
		.await?
		.into())
	}
	async fn retire_collections(&mut self, thread: Uuid) -> Result<()> {
		SemanticCollection::retire_thread(self.tx()?, thread)
			.await
			.map_err(Into::into)
	}
	async fn record_collection(
		&mut self,
		thread: Uuid,
		collection: &str,
		vector: Value,
	) -> Result<()> {
		SemanticCollection::record_generation(self.tx()?, collection, thread, vector)
			.await
			.map_err(Into::into)
	}
	async fn active(&mut self, thread: Uuid) -> Result<Vec<Uuid>> {
		Ok(SemanticEntry::lock_active(self.tx()?, thread)
			.await?
			.into_iter()
			.map(|entry| entry.id)
			.collect())
	}
	async fn reconfigure(&mut self, id: Uuid, revision: i64) -> Result<Entry> {
		Ok(SemanticEntry::reconfigure(self.tx()?, id, revision)
			.await?
			.into())
	}
	async fn schedule(&mut self, entry: &Entry, collection: &str) -> Result<()> {
		SemanticPoint::schedule(self.tx()?, &native::Entry::from(entry.clone()), collection)
			.await
			.map_err(Into::into)
	}
	async fn rebuild_images(&mut self, thread: Uuid, max_images: usize) -> Result<()> {
		let lease = self
			.lease
			.as_mut()
			.ok_or_else(|| Error::Conflict("semantic index configuration ended".into()))?;
		super::attachment_images::relimit(lease, thread, max_images).await?;
		// Images posted before the index existed or was enabled enter it now,
		// under the authority of the person configuring it (#622).
		super::attachment_images::backfill(lease, thread, max_images)
			.await
			.map_err(Into::into)
	}
	async fn backfill_memory(&mut self, thread: Uuid) -> Result<()> {
		let lease = self
			.lease
			.as_mut()
			.ok_or_else(|| Error::Conflict("semantic index configuration ended".into()))?;
		super::native_memory::backfill_builtin(lease, thread)
			.await
			.map_err(Into::into)
	}
	async fn memory_embeddings(&mut self, thread: Uuid) -> Result<i64> {
		use aidash_domain::registry::bindings::memory_defaults;
		// Explicit providers pin their embedding and are refused by `replace`.
		Ok(crate::database::native::query_scalar(
			&Query::select()
				.expr(Func::count(Expr::col(("e", "id")).into()))
				.from_as(Alias::new("semantic_entries"), Alias::new("e"))
				.inner_join(
					Alias::new("memory_units"),
					Expr::col(("e", "id")).equals(("memory_units", "id")),
				)
				.inner_join(
					Alias::new("memory_bank_settings"),
					Expr::col(("memory_units", "bank_id"))
						.equals(("memory_bank_settings", "bank_id")),
				)
				.inner_join(
					Alias::new("semantic_indexes"),
					Expr::col(("e", "thread_id")).equals(("semantic_indexes", "thread_id")),
				)
				.inner_join(
					Alias::new("semantic_points"),
					Expr::col(("e", "point_id")).equals(("semantic_points", "id")),
				)
				.and_where(Expr::col(("e", "thread_id")).eq(Expr::value(thread)))
				.and_where(
					Expr::col(("e", "index_revision")).equals(("semantic_indexes", "revision")),
				)
				.and_where(Expr::col(("semantic_points", "retired")).eq(false))
				.and_where(Expr::col(("semantic_points", "content_digest")).is_not_null())
				.and_where(Expr::col(("e", "deleted")).eq(false))
				.and_where(
					Expr::col(("memory_bank_settings", "provider_id"))
						.eq(memory_defaults::MEMORY_ID),
				)
				.and_where(
					Expr::col(("memory_bank_settings", "provider_version"))
						.eq(memory_defaults::VERSION),
				)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(self.tx()?)
		.await?)
	}
	async fn history(&mut self, thread: Uuid, revision: i64) -> Result<()> {
		SemanticHistory::append(
			self.tx()?,
			thread,
			None,
			revision,
			"CONFIGURED",
			"new immutable index generation",
		)
		.await
		.map_err(Into::into)
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.lease
			.take()
			.ok_or_else(|| Error::Conflict("semantic index configuration ended".into()))?
			.finish(Ok(()))
			.await
			.map_err(Into::into)
	}
}
impl aidash_application::ports::authorization::visibility::RowOutcome for Entries<'_, '_> {
	fn begin_row(&mut self) {
		self.lease.begin_row();
	}
	fn row_silence(&self) -> Option<String> {
		self.lease.row_silence()
	}
	fn take_row_silence(&mut self) -> Option<String> {
		self.lease.take_row_silence()
	}
}
#[async_trait]
impl SemanticEntriesSession for Entries<'_, '_> {
	async fn thread(&mut self, thread: Uuid, action: &str) -> Result<()> {
		self.lease.thread(thread, action).await.map_err(Into::into)
	}
	async fn index(&mut self, thread: Uuid, exclusive: bool) -> Result<Index> {
		Ok(service::index(self.lease.tx(), thread, exclusive)
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
	async fn source(&mut self, thread: Uuid, source: &Source) -> Result<Option<String>> {
		self.lease.source(thread, source).await.map_err(Into::into)
	}
	async fn image_visible(&mut self, thread: Uuid, source: &Source) -> Result<bool> {
		if !matches!(source, Source::AttachmentImage { .. }) {
			return Ok(false);
		}
		super::attachment_images::visible(self.lease, thread, source)
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
		thread: Uuid,
		id: Uuid,
		revision: i64,
		state: &str,
		detail: &str,
	) -> Result<()> {
		service::history(self.lease.tx(), thread, Some(id), revision, state, detail)
			.await
			.map_err(Into::into)
	}

	async fn by_key(&mut self, thread: Uuid, key: &str) -> Result<Option<Entry>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Option<native::Entry>> = async {
			let value: Option<native::Entry> = {
				let query_bind_1 = thread;
				let query_bind_2 = key;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(thread_id = ? AND key = ?)".to_owned(),
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

	async fn count(&mut self, thread: Uuid) -> Result<i64> {
		let lease = &mut *self.lease;

		let result: NativeResult<i64> = async {
			let value: i64 = {
				let query_bind_1 = thread;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(Expr::cust("COUNT(*)"))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(thread_id = ? AND NOT deleted AND source->>'kind' NOT IN ('working_image', 'attachment_image'))".to_owned(),
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
		let thread = entry.thread_id;
		let saved = authority;
		let result: NativeResult<native::Entry> = async {
			let value: native::Entry = {
				let query_bind_1 = entry.id;
				let query_bind_2 = thread;
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
							Alias::new("thread_id"),
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
							Alias::new("tenant"),
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
								.expr(crate::database::scope::tenant_of(
									"threads",
									"id",
									query_bind_2,
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
					"thread_id",
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

	async fn list(&mut self, thread: Uuid) -> Result<Vec<Entry>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Vec<native::Entry>> = async {
			let value: Vec<native::Entry> = {
				let query_bind_1 = thread;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(thread_id = ? AND NOT deleted)".to_owned(),
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

	async fn lock_entry(&mut self, thread: Uuid, id: Uuid) -> Result<Option<Entry>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Option<native::Entry>> = async {
			let value: Option<native::Entry> = {
				let query_bind_1 = thread;
				let query_bind_2 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("semantic_entries"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(thread_id = ? AND id = ?)".to_owned(),
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
		thread: Uuid,
		id: Uuid,
		point: Uuid,
		index_revision: i64,
		delete: bool,
		authority: Value,
	) -> Result<Entry> {
		let lease = &mut *self.lease;
		let saved = authority;
		let result: NativeResult<native::Entry>=async { let value: native::Entry={ let query_bind_1 = id; let query_bind_2 = thread; let query_bind_3 = point; let query_bind_4 = index_revision; let query_bind_5 = delete; let query_bind_6 = if delete {"DELETED"} else {"PENDING"}; let query_bind_7 = saved; crate::database::native::query_as(&Query::update().table(Alias::new("semantic_entries")).value_expr(Alias::new("revision"), Expr::cust("revision + 1")).value_expr(Alias::new("point_id"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(Alias::new("index_revision"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.to_owned()).into()])).value_expr(Alias::new("deleted"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(Alias::new("state"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).value_expr(Alias::new("source"), SimpleExpr::CustomWithExpr("(CASE WHEN ? THEN JSONB_BUILD_OBJECT('kind', 'memory', 'text', '') ELSE source END)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(Alias::new("authority"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_7.to_owned()).into()])).value_expr(Alias::new("attempts"), Expr::cust("0")).value_expr(Alias::new("last_error"), Expr::cust("NULL")).value_expr(Alias::new("next_attempt"), Expr::cust("CLOCK_TIMESTAMP()")).value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()")).and_where(SimpleExpr::CustomWithExpr("(id = ? AND thread_id = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()])).returning_all().to_string(PostgresQueryBuilder)).fetch_one(&mut **lease.tx()).await? }; Ok(value) }.await;
		result.map(|value| value.into()).map_err(Into::into)
	}

	async fn history_page(&mut self, thread: Uuid, cursor: i64) -> Result<Vec<History>> {
		let lease = &mut *self.lease;

		let result: NativeResult<Vec<native::History>> = async {
			let value: Vec<native::History> = {
				let query_bind_1 = thread;
				let query_bind_2 = cursor;
				crate::database::native::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("semantic_history"))
						.and_where(
							Expr::col(Alias::new("thread_id"))
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
			thread_id: value.thread_id,
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
			thread_id: value.thread_id,
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
			thread_id: value.thread_id,
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
			thread_id: value.thread_id,
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
			thread_id: value.thread_id,
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
			thread_id: value.thread_id,
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
			max_image_bytes: value.max_image_bytes,
			max_images: value.max_images,
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
			max_image_bytes: value.max_image_bytes,
			max_images: value.max_images,
		}
	}
}
