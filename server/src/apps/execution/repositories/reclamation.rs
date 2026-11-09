//! Native maintenance transactions retain every object and quota ownership predicate.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::services::{
	python,
	records::{self, Record as NativeRecord},
	sessions,
};
use crate::{Result as NativeResult, store::Store};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::{
	Result,
	ports::capabilities::reclamation::{ReclamationRepository, ReclamationScope},
};
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, LockBehavior, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

/// The capability worker reaches every Tenant's operations and work areas.
const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
/// A superseded object may be reclaimed only after its active holds expire.
pub(crate) fn no_active_hold() -> SimpleExpr {
	Expr::not_exists(
		Query::select()
			.expr(Expr::value(1))
			.from_as(Alias::new("core_retention_files"), Alias::new("f"))
			.join(
				reinhardt::query::JoinType::InnerJoin,
				reinhardt::query::TableRef::table_alias(
					Alias::new("core_retention_holds"),
					Alias::new("h"),
				),
				Expr::col((Alias::new("h"), Alias::new("id")))
					.equals((Alias::new("f"), Alias::new("hold_id"))),
			)
			.and_where(
				Expr::col((Alias::new("f"), Alias::new("file_id")))
					.equals((Alias::new("core_objects"), Alias::new("id"))),
			)
			.and_where(
				Expr::col((Alias::new("f"), Alias::new("tenant")))
					.equals((Alias::new("core_objects"), Alias::new("tenant"))),
			)
			.cond_where(
				Condition::any()
					.add(Expr::col((Alias::new("h"), Alias::new("expires_at"))).is_null())
					.add(
						Expr::col((Alias::new("h"), Alias::new("expires_at")))
							.gt(Expr::current_timestamp()),
					),
			)
			.to_owned(),
	)
	.into()
}
pub(crate) fn no_revision_references() -> reinhardt::query::SimpleExpr {
	Expr::not_exists(
		Query::select()
			.expr(Expr::value(1))
			.from(Alias::new("core_blob_references"))
			.and_where(
				Expr::col((Alias::new("core_blob_references"), Alias::new("object_id")))
					.equals((Alias::new("core_objects"), Alias::new("id"))),
			)
			.and_where(
				Expr::col((Alias::new("core_blob_references"), Alias::new("tenant")))
					.equals((Alias::new("core_objects"), Alias::new("tenant"))),
			)
			.to_owned(),
	)
	.into()
}
/// A superseded working object, a receipt or transfer snapshot, or a Thread
/// Attachment object (or its WAV rendition) whose attachment row of the same
/// Tenant was deleted. Callers still require no hold and no revision reference.
fn reclaimable_working() -> Condition {
	let kind = |value: &str| {
		SimpleExpr::from(Expr::col((Alias::new("core_objects"), Alias::new("kind"))))
			.eq(Expr::value(value.to_owned()))
	};
	let unreferenced = |column: &str| {
		Expr::not_exists(
			Query::select()
				.expr(Expr::value(1_i32))
				.from_as(Alias::new("channel_attachments"), Alias::new("a"))
				.and_where(
					SimpleExpr::from(Expr::col((Alias::new("a"), Alias::new("tenant")))).eq(
						Expr::col((Alias::new("core_objects"), Alias::new("tenant"))),
					),
				)
				.and_where(
					SimpleExpr::from(Expr::col((Alias::new("a"), Alias::new(column))))
						.eq(Expr::col((Alias::new("core_objects"), Alias::new("id")))),
				)
				.to_owned(),
		)
	};
	Condition::any()
		.add(kind("superseded_working"))
		.add(
			Condition::all()
				.add(kind("thread_attachment"))
				.add(unreferenced("object_id")),
		)
		.add(
			Condition::all()
				.add(kind("thread_attachment_wav"))
				.add(unreferenced("wav_object_id")),
		)
		.add(kind("received"))
		.add(kind("transfer_snapshot"))
}
pub(crate) struct Repository<'a> {
	store: &'a Store,
	orphan_cursor: tokio::sync::Mutex<Option<tokio::fs::ReadDir>>,
}
impl<'a> Repository<'a> {
	pub(crate) fn new(store: &'a Store) -> Self {
		Self {
			store,
			orphan_cursor: tokio::sync::Mutex::new(None),
		}
	}
}
struct Scope<'a> {
	store: &'a Store,
	tx: crate::database::native::Transaction,
}
#[async_trait]
impl ReclamationRepository for Repository<'_> {
	async fn begin(&self) -> Result<Box<dyn ReclamationScope + '_>> {
		Ok(Box::new(Scope {
			store: self.store,
			tx: self.store.pool.begin(&MAINTENANCE).await?,
		}))
	}
	async fn retained(&self, after: Uuid) -> Result<Vec<Uuid>> {
		let result:NativeResult<Vec<Uuid>>=async {
let ids: Vec<Uuid> = {
			let query_bind_1 = after;
			crate::database::native::query_scalar(&Query::select().column(Alias::new("id")).from(Alias::new("core_records"))
            .and_where(Expr::col(Alias::new("kind")).is_in(["transfer_in", "transfer_out", "reference"]))
            .and_where(Expr::cust("COALESCE(data->>'objects_released','false') <> 'true'"))
            .and_where(Expr::col(Alias::new("data")).binary(reinhardt::query::BinOper::PgOperator(reinhardt::query::types::PgBinOper::Contains), Expr::value(serde_json::json!({"receipt_retained": true}))).not())
            .and_where(Condition::any()
                .add(Expr::cust("kind = 'transfer_out' AND (state IN ('delivered','blocked') OR expires_at <= CURRENT_TIMESTAMP)"))
                .add(Expr::cust("kind = 'transfer_in' AND (state = 'committed' OR expires_at <= CURRENT_TIMESTAMP)"))
                .add(Expr::cust("kind = 'reference' AND (state = 'revoked' OR expires_at <= CURRENT_TIMESTAMP OR (state IN ('ready','extracting') AND jsonb_array_length(data->'chunks') > 0))")))
            .and_where(Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())))
            .order_by(Alias::new("id"), reinhardt::query::Order::Asc)
            .limit(8).to_string(PostgresQueryBuilder)).scalar_all(self.store.pool.in_scope(&MAINTENANCE)).await?
		};
        Ok(ids)
    }.await;
		result.map_err(Into::into)
	}
	async fn working(&self, after: Uuid) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let ids: Vec<Uuid> = {
				let query_bind_1 = after;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_objects"))
						.cond_where(reclaimable_working())
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(no_active_hold())
						.and_where(no_revision_references())
						.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
						.limit(16)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(self.store.pool.in_scope(&MAINTENANCE))
				.await?
			};
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn expire_holds(&self) -> Result<()> {
		let result: NativeResult<()> = async {
			crate::database::native::query(
				&Query::delete()
					.from_table(Alias::new("core_retention_holds"))
					.and_where(Expr::cust(
						"expires_at IS NOT NULL AND expires_at <= CURRENT_TIMESTAMP",
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(self.store.pool.in_scope(&MAINTENANCE))
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn expire_previews(&self) -> Result<()> {
		let result: NativeResult<()> = async {
			for table in ["core_preview_tickets", "core_preview_sessions"] {
				crate::database::native::query(
					&Query::delete()
						.from_table(Alias::new(table))
						.and_where(Expr::cust("expires_at <= CURRENT_TIMESTAMP"))
						.to_string(PostgresQueryBuilder),
				)
				.execute(self.store.pool.in_scope(&MAINTENANCE))
				.await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn python(&self, cursor: &mut Uuid) -> Result<()> {
		python::reap(self.store, cursor).await.map_err(Into::into)
	}
	async fn orphans(&self) -> Result<()> {
		prune_revisions(self.store).await?;
		let mut cursor = self.orphan_cursor.lock().await;
		let reconciliation = self
			.store
			.capabilities
			.reconcile_orphan_batch(&self.store.pool, &mut cursor)
			.await;
		drop(cursor);
		let reclamation = reclaim_unreferenced_blobs(self.store).await;
		reconciliation?;
		reclamation.map_err(Into::into)
	}
}

/// An Area row lock serializes pruning with creation of a retention hold.
async fn prune_revisions(store: &Store) -> NativeResult<()> {
	let held = || {
		Expr::exists(
			Query::select()
				.expr(Expr::value(1))
				.from(Alias::new("core_retention_holds"))
				.and_where(
					Expr::col((Alias::new("core_retention_holds"), Alias::new("tenant")))
						.equals((Alias::new("core_manifest_revisions"), Alias::new("tenant"))),
				)
				.and_where(
					Expr::col((Alias::new("core_retention_holds"), Alias::new("area_id")))
						.equals((Alias::new("core_manifest_revisions"), Alias::new("area_id"))),
				)
				.and_where(
					Expr::col((Alias::new("core_retention_holds"), Alias::new("revision"))).equals(
						(
							Alias::new("core_manifest_revisions"),
							Alias::new("revision"),
						),
					),
				)
				.cond_where(
					Condition::any()
						.add(
							Expr::col((
								Alias::new("core_retention_holds"),
								Alias::new("expires_at"),
							))
							.is_null(),
						)
						.add(
							Expr::col((
								Alias::new("core_retention_holds"),
								Alias::new("expires_at"),
							))
							.gt(Expr::current_timestamp()),
						),
				)
				.to_owned(),
		)
	};
	let candidates: Vec<(String, Uuid)> = crate::database::native::query_as(
		&Query::select()
			.columns(["tenant", "id"].map(Alias::new))
			.from(Alias::new("core_areas"))
			.and_where(Expr::exists(
				Query::select()
					.expr(Expr::value(1))
					.from(Alias::new("core_manifest_revisions"))
					.and_where(
						Expr::col((Alias::new("core_manifest_revisions"), Alias::new("tenant")))
							.equals((Alias::new("core_areas"), Alias::new("tenant"))),
					)
					.and_where(
						Expr::col((Alias::new("core_manifest_revisions"), Alias::new("area_id")))
							.equals((Alias::new("core_areas"), Alias::new("id"))),
					)
					.and_where(
						Expr::col((
							Alias::new("core_manifest_revisions"),
							Alias::new("revision"),
						))
						.ne(Expr::col((
							Alias::new("core_areas"),
							Alias::new("revision"),
						))),
					)
					.and_where(held().not())
					.to_owned(),
			))
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["tenant", "id"])
	.fetch_all(store.pool.in_scope(&MAINTENANCE))
	.await?;
	for (tenant, area) in candidates {
		let mut tx = store.pool.begin(&MAINTENANCE).await?;
		let current: Option<i64> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("revision"))
				.from(Alias::new("core_areas"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&tenant)))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(area)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *tx)
		.await?;
		if let Some(current) = current {
			crate::database::native::query(
				&Query::delete()
					.from_table(Alias::new("core_manifest_revisions"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(&tenant)))
					.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(area)))
					.and_where(Expr::col(Alias::new("revision")).ne(Expr::value(current)))
					.and_where(held().not())
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
	}
	Ok(())
}

/// Blobs nothing names any more: no object, reference or upload.
fn unreferenced_blobs_query() -> reinhardt::query::SelectStatement {
	let mut query = Query::select();
	query.from(Alias::new("core_blobs"));
	for table in ["core_objects", "core_blob_references", "core_blob_uploads"] {
		query.and_where(Expr::not_exists(
			Query::select()
				.expr(Expr::value(1))
				.from(Alias::new(table))
				.and_where(
					Expr::col((Alias::new(table), Alias::new("tenant")))
						.equals((Alias::new("core_blobs"), Alias::new("tenant"))),
				)
				.and_where(
					Expr::col((Alias::new(table), Alias::new("digest")))
						.equals((Alias::new("core_blobs"), Alias::new("digest"))),
				)
				.to_owned(),
		));
	}
	query
}
/// One Tenant's unreferenced digests, for a purge that just released its objects.
pub(crate) async fn unreferenced_blobs(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	limit: u64,
) -> crate::Result<Vec<String>> {
	let (sql, values) = unreferenced_blobs_query()
		.column(Alias::new("digest"))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
		.limit(limit)
		.build(PostgresQueryBuilder);
	tx.fetch_all(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await?
		.into_iter()
		.map(|row| {
			row.get("digest")
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(Into::into)
		})
		.collect()
}

/// Reap only blobs whose object and reference removals already committed.
async fn reclaim_unreferenced_blobs(store: &Store) -> NativeResult<()> {
	let candidates: Vec<(String, String)> = crate::database::native::query_as(
		&unreferenced_blobs_query()
			.columns(["tenant", "digest"].map(Alias::new))
			.limit(16)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["tenant", "digest"])
	.fetch_all(store.pool.in_scope(&MAINTENANCE))
	.await?;
	for (tenant, digest) in candidates {
		let mut tx = store.pool.begin(&MAINTENANCE).await?;
		reclaim_blob(store, &mut tx, &tenant, &digest).await?;
		tx.commit().await?;
	}
	Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlobReclamation {
	Reclaimed,
	Retained,
	Busy,
}

/// Reclaim an already unreferenced committed blob under the publication lock.
pub(crate) async fn reclaim_blob(
	store: &Store,
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	digest: &str,
) -> NativeResult<BlobReclamation> {
	let lock = format!("core-blob:{tenant}:{digest}");
	let acquired: bool = crate::database::native::query_scalar(
		&Query::select()
			.expr(SimpleExpr::CustomWithExpr(
				"(pg_try_advisory_xact_lock(hashtextextended(?, 0)))".into(),
				vec![Expr::value(lock).into()],
			))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **tx)
	.await?;
	if !acquired {
		return Ok(BlobReclamation::Busy);
	}
	// A new logical reference takes a foreign-key key-share lock on this row.
	// Lock it before checking references and deleting bytes so that publication
	// cannot commit a reference between the check and physical deletion.
	let size: Option<i64> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("size"))
			.from(Alias::new("core_blobs"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
			.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(digest)))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **tx)
	.await?;
	let Some(size) = size else {
		// SKIP LOCKED also returns no row when another transaction owns it.
		let exists: Option<i64> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("size"))
				.from(Alias::new("core_blobs"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(digest)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **tx)
		.await?;
		return Ok(if exists.is_some() {
			BlobReclamation::Busy
		} else {
			BlobReclamation::Retained
		});
	};
	let mut referenced = false;
	for table in ["core_objects", "core_blob_references", "core_blob_uploads"] {
		let id: Option<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new(table))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(digest)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **tx)
		.await?;
		referenced |= id.is_some();
	}
	if !referenced {
		let parsed = digest
			.parse()
			.map_err(|_| crate::Error::Conflict("OBJECT_INTEGRITY".into()))?;
		store
			.capabilities
			.blob_store()
			.await?
			.delete(tenant, parsed)
			.await?;
		crate::database::native::query(
			&Query::delete()
				.from_table(Alias::new("core_blobs"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(digest)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		super::capability_objects::lock_tenant_quota(tx, tenant).await?;
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("core_quotas"))
				.value_expr(
					Alias::new("used_bytes"),
					Expr::col(Alias::new("used_bytes")).sub(Expr::value(size)),
				)
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		return Ok(BlobReclamation::Reclaimed);
	}
	Ok(BlobReclamation::Retained)
}

#[async_trait]
impl ReclamationScope for Scope<'_> {
	async fn load(&mut self, id: Uuid) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let record: NativeRecord = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&sessions::select("core_records")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut *self.tx)
				.await?
			};
			Ok(record)
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
	async fn release_quota(&mut self, tenant: &str, reserved: i64) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = tenant;
				let query_bind_2 = reserved;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_quotas"))
						.value_expr(
							Alias::new("reserved_bytes"),
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"reserved_bytes",
							)))
							.sub(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"reserved_bytes",
							)))
							.gte(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *self.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn release_transfer_refs(&mut self, tenant: &str, id: Uuid) -> Result<()> {
		let result: NativeResult<()> = async {
			crate::database::native::query(
				&Query::delete()
					.from_table(Alias::new("core_blob_references"))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.and_where(Expr::col(Alias::new("record_id")).eq(Expr::value(id)))
					.and_where(Expr::col(Alias::new("holder_kind")).eq(Expr::value("receipt")))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *self.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn object_kind(&mut self, id: Uuid, tenant: &str) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let kind: Option<String> = {
				let query_bind_1 = id;
				let query_bind_2 = tenant;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("kind"))
						.from(Alias::new("core_objects"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							),
						)
						.and_where(Expr::col(Alias::new("area_id")).is_null())
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut *self.tx)
				.await?
			};
			Ok(kind)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn object_named(&mut self, tenant: &str, id: Uuid) -> Result<bool> {
		// The object row fences new FK references until the caller's erase.
		// Do not wait behind a publisher that may itself need this record.
		let object = || {
			Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.to_owned()
		};
		let locked: Option<Uuid> = crate::database::native::query_scalar(
			&object()
				.lock(LockType::Update)
				.lock_behavior(LockBehavior::SkipLocked)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *self.tx)
		.await?;
		if locked.is_none() {
			let existing: Option<Uuid> =
				crate::database::native::query_scalar(&object().to_string(PostgresQueryBuilder))
					.scalar_optional(&mut *self.tx)
					.await?;
			return Ok(existing.is_some());
		}
		for (table, column) in [
			("core_blob_references", "object_id"),
			("core_retention_files", "file_id"),
		] {
			let found: Option<Uuid> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new(column))
					.from(Alias::new(table))
					.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
					.and_where(Expr::col(Alias::new(column)).eq(Expr::value(id)))
					.limit(1)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *self.tx)
			.await?;
			if found.is_some() {
				return Ok(true);
			}
		}
		Ok(false)
	}
	async fn working_tenant(&mut self, id: Uuid) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			let tenant: Option<String> = {
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("tenant"))
						.from(Alias::new("core_objects"))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.cond_where(reclaimable_working())
						// A hold written between the selection and this lock refuses the
						// erase instead of dropping the bytes a reference still names.
						.and_where(no_active_hold())
						.and_where(no_revision_references())
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut *self.tx)
				.await?
			};
			Ok(tenant)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn erase(&mut self, tenant: &str, id: Uuid) -> Result<()> {
		self.store
			.capabilities
			.erase_committed(&mut self.tx, tenant, id, false)
			.await
			.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut current = native(record);
		records::update_committed(&mut self.tx, &mut current).await?;
		record.revision = current.revision;
		Ok(())
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		use aidash_application::ports::capabilities::runner::RunnerTransport as _;
		crate::bootstrap::operation_runner(self.store)?
			.request(method, path, body.as_ref())
			.await
	}
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()> {
		let Scope { tx, .. } = *self;
		match result {
			Ok(true) => tx.commit().await.map_err(|error| error.into()),
			Ok(false) => Ok(()),
			Err(error) => Err(error),
		}
	}
}
