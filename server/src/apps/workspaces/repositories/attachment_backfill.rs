//! Move legacy inline attachment bytes to the immutable capability store.
use crate::{Result, store::Store};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);

struct LegacyAttachment {
	id: Uuid,
	tenant: String,
	sha256: String,
	size_bytes: i64,
	content: Vec<u8>,
}
crate::native_record!(LegacyAttachment {
	id,
	tenant,
	sha256,
	size_bytes,
	content
});

/// Each row and object share a transaction; abandoned filesystem writes are
/// covered by the object store's durable intents and orphan reconciler.
pub(crate) async fn run(store: &Store) -> Result<()> {
	let mut count = 0_u64;
	loop {
		let mut tx = store.pool.begin(&MAINTENANCE).await?;
		let query = Query::select()
			.columns(["id", "tenant", "sha256", "size_bytes", "content"].map(Alias::new))
			.from(Alias::new("channel_attachments"))
			.and_where(Expr::col(Alias::new("object_id")).is_null())
			.and_where(Expr::col(Alias::new("content")).is_not_null())
			.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
			.limit(1)
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder);
		let row: Option<LegacyAttachment> = crate::database::native::query_as(&query)
			.columns(&["id", "tenant", "sha256", "size_bytes", "content"])
			.fetch_optional(&mut *tx)
			.await?;
		let Some(row) = row else {
			tx.commit().await?;
			break;
		};
		if i64::try_from(row.content.len()).ok() != Some(row.size_bytes) {
			return Err(crate::Error::Conflict("OBJECT_INTEGRITY".into()));
		}
		let mut pending = store
			.capabilities
			.begin_object_for(
				&mut tx,
				&row.tenant,
				None,
				"thread_attachment",
				row.content.len() as u64,
				true,
			)
			.await?;
		pending.write_block(&row.content).await?;
		let (object_id, _) = pending
			.finish_for(&mut tx, &row.tenant, Some(&row.sha256))
			.await?;
		let update = Query::update()
			.table(Alias::new("channel_attachments"))
			.value(Alias::new("object_id"), object_id)
			.value_expr(Alias::new("content"), Expr::cust("NULL"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(row.id)))
			.to_string(PostgresQueryBuilder);
		crate::database::native::query(&update)
			.execute(&mut *tx)
			.await?;
		tx.commit().await?;
		count += 1;
		if count.is_multiple_of(100) {
			tracing::info!(count, "migrated legacy attachments");
		}
	}
	if count > 0 {
		tracing::info!(count, "legacy attachment migration completed");
	}
	Ok(())
}
