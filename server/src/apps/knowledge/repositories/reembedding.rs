//! Build replacement vectors under each source's saved authority. Publish a
//! Tenant's new Thread generations and active model in one transaction.
use super::{indexing::reembed, postgres_vector::vector};
use crate::{
	Error, Result,
	apps::{
		identity::repositories::embedding_settings,
		knowledge::{
			models::{
				SemanticCollection, SemanticEntry, SemanticHistory, SemanticIndexe, SemanticPoint,
			},
			serializers::contracts::{Entry, Index},
		},
		workspaces::models::Thread,
	},
	database::native::{self, Transaction},
	store::Store,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::semantic::indexing::IndexingSpec;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, IntoIden, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

const JOB_LOCK: i64 = 1_019_022;

pub(crate) async fn sweep(store: &Store) -> Result<()> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::SemanticMaintenance);
	let sql = Query::select()
		.column(Alias::new("tenant"))
		.from(Alias::new("tenant_embedding_settings"))
		.and_where(Expr::col("pending").is_not_null())
		.and_where(Expr::col("next_attempt").lte(Expr::cust("clock_timestamp()")))
		.order_by(Alias::new("next_attempt"), Order::Asc)
		.limit(8)
		.to_string(PostgresQueryBuilder);
	let tenants: Vec<String> = native::query_scalar(&sql)
		.scalar_all(store.pool.in_scope(&scope))
		.await?;
	for tenant in tenants {
		if let Err(error) = Box::pin(rebuild(store, &tenant)).await {
			tracing::warn!(%tenant, %error, "tenant re-embedding is pending");
		}
	}
	Ok(())
}

async fn rebuild(store: &Store, tenant: &str) -> Result<()> {
	let scope = TenancyScope::tenant(tenant)?;
	let mut tx = store.pool.begin(&scope).await?;
	let lock = SimpleExpr::FunctionCall(
		"pg_try_advisory_xact_lock".into_iden(),
		vec![SimpleExpr::FunctionCall(
			"hashtextextended".into_iden(),
			vec![Expr::value(tenant).into(), Expr::value(JOB_LOCK).into()],
		)],
	);
	let sql = Query::select().expr(lock).to_string(PostgresQueryBuilder);
	if !native::query_scalar::<bool>(&sql)
		.scalar_one(&mut *tx)
		.await?
	{
		return Ok(());
	}
	let settings = embedding_settings::read(&mut *tx, tenant).await?;
	let Some(target) = &settings.pending else {
		return Ok(());
	};
	// Throttle repeated failure and leave a durable request after a worker crash.
	embedding_settings::defer(store, tenant, settings.revision).await?;
	if !embedding_settings::drive_reembedding_ready(&mut *tx, tenant, target).await? {
		return Ok(());
	}
	let embedding = embedding_settings::embedding_config(&mut *tx, tenant, target).await?;
	let indexes = tenant_indexes(&mut *tx, tenant).await?;
	let mut prepared = Vec::with_capacity(indexes.len());
	for index in indexes {
		let mut spec: IndexingSpec = serde_json::from_value(index.spec.clone())?;
		spec.embedding = embedding.clone();
		crate::apps::knowledge::repositories::mutations::validate_index(store, &spec)?;
		prepared.push(Box::pin(stage(store, &mut tx, index, spec)).await?);
	}

	embedding_settings::lock_tenant(&mut *tx, tenant, LockType::Update).await?;
	let current = embedding_settings::read(&mut *tx, tenant).await?;
	if current.revision != settings.revision
		|| !embedding_settings::drive_reembedding_ready(&mut *tx, tenant, target).await?
	{
		return Ok(());
	}
	let current_indexes = tenant_indexes(&mut *tx, tenant).await?;
	if current_indexes.len() != prepared.len()
		|| current_indexes
			.iter()
			.zip(&prepared)
			.any(|(current, plan)| {
				current.thread_id != plan.index.thread_id || current.revision != plan.index.revision
			}) {
		return Err(Error::Conflict(
			"Tenant indexes changed during re-embedding".into(),
		));
	}
	for plan in prepared {
		publish(&mut *tx, plan).await?;
	}
	embedding_settings::activate(&mut *tx, tenant, settings.revision, target).await?;
	tx.commit().await
}

async fn tenant_indexes(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Vec<Index>> {
	let sql = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new("semantic_indexes"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(embedding_settings::live_thread("semantic_indexes"))
		.order_by(Alias::new("thread_id"), Order::Asc)
		.to_string(PostgresQueryBuilder);
	Ok(native::query_as(&sql).fetch_all(tx).await?)
}

struct Point {
	entry: Entry,
	id: Uuid,
	digest: Option<String>,
}
struct Generation {
	index: Index,
	spec: IndexingSpec,
	collection: String,
	revision: i64,
	points: Vec<Point>,
}

async fn stage(
	store: &Store,
	tx: &mut Transaction,
	index: Index,
	spec: IndexingSpec,
) -> Result<Generation> {
	let collection = format!("aidash_{}", Uuid::new_v4().simple());
	let revision = index
		.revision
		.checked_add(1)
		.ok_or_else(|| Error::Conflict("Thread index revision exhausted".into()))?;
	let sql = Query::insert()
		.into_table(Alias::new("semantic_vector_collections"))
		.columns(["collection", "dimensions"].map(Alias::new))
		.values_panic([
			Expr::value(&collection),
			Expr::value(spec.embedding.dimensions as i32),
		])
		.to_string(PostgresQueryBuilder);
	native::query(&sql).execute(&mut **tx).await?;
	let sql = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new("semantic_entries"))
		.and_where(Expr::col("tenant").eq(index.tenant.as_str()))
		.and_where(Expr::col("thread_id").eq(Expr::value(index.thread_id)))
		.and_where(Expr::col("deleted").eq(false))
		.order_by(Alias::new("id"), Order::Asc)
		.to_string(PostgresQueryBuilder);
	let entries: Vec<Entry> = native::query_as(&sql).fetch_all(&mut **tx).await?;
	let mut points = Vec::with_capacity(entries.len());
	for entry in entries {
		let id = Uuid::new_v4();
		let mut digest = None;
		if spec.enabled
			&& entry.state == "READY"
			&& let Some((data, content_digest)) =
				Box::pin(reembed(store, &index.tenant, &entry, &spec)).await?
		{
			if data.len() != spec.embedding.dimensions {
				return Err(Error::Invalid(
					"replacement embedding dimension mismatch".into(),
				));
			}
			let payload = json!({
				"entry_id": entry.id, "revision": entry.revision,
				"index_revision": revision, "thread_id": entry.thread_id, "tenant": index.tenant,
			});
			let sql = Query::insert()
				.into_table(Alias::new("semantic_vectors"))
				.columns(
					[
						"id",
						"collection",
						"thread_id",
						"tenant",
						"embedding",
						"payload",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(id))
						.expr(Expr::value(&collection))
						.expr(Expr::value(entry.thread_id))
						.expr(Expr::value(&index.tenant))
						.expr(vector(&data)?)
						.expr(Expr::value(payload))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder);
			native::query(&sql).execute(&mut **tx).await?;
			digest = Some(content_digest);
		}
		points.push(Point { entry, id, digest });
	}
	Ok(Generation {
		index,
		spec,
		collection,
		revision,
		points,
	})
}

async fn publish(tx: &mut dyn TransactionExecutor, plan: Generation) -> Result<()> {
	let thread = plan.index.thread_id;
	Thread::lock_existing(tx, thread).await?;
	let current = SemanticIndexe::locked(tx, thread, true).await?;
	if current.is_none_or(|index| index.revision != plan.index.revision) {
		return Err(Error::Conflict(
			"Thread index changed during re-embedding".into(),
		));
	}
	let entries = SemanticEntry::lock_active(tx, thread).await?;
	if entries.len() != plan.points.len()
		|| entries.iter().zip(&plan.points).any(|(current, staged)| {
			let entry = &staged.entry;
			current.id != entry.id
				|| current.revision != entry.revision
				|| current.point_id != entry.point_id
				|| current.state != entry.state
		}) {
		return Err(Error::Conflict(
			"Thread sources changed during re-embedding".into(),
		));
	}
	SemanticCollection::retire_thread(tx, thread).await?;
	SemanticIndexe::replace_generation(
		tx,
		thread,
		&plan.index.tenant,
		plan.revision,
		serde_json::to_value(&plan.spec)?,
		&plan.collection,
	)
	.await?;
	SemanticCollection::record_generation(
		tx,
		&plan.collection,
		thread,
		serde_json::to_value(&plan.spec.vector)?,
	)
	.await?;
	for Point {
		mut entry,
		id,
		digest,
	} in plan.points
	{
		entry.point_id = id;
		let state = if digest.is_some() { "READY" } else { "PENDING" };
		let sql = Query::update()
			.table(Alias::new("semantic_entries"))
			.value_expr(Alias::new("point_id"), Expr::value(id))
			.value_expr(Alias::new("index_revision"), Expr::value(plan.revision))
			.value_expr(Alias::new("state"), Expr::value(state))
			.value_expr(Alias::new("attempts"), Expr::value(0))
			.value_expr(Alias::new("next_attempt"), Expr::cust("clock_timestamp()"))
			.value_expr(Alias::new("updated_at"), Expr::cust("clock_timestamp()"))
			.value_expr(Alias::new("last_error"), Expr::null())
			.and_where(Expr::col("id").eq(Expr::value(entry.id)))
			.to_string(PostgresQueryBuilder);
		native::query(&sql).execute(&mut *tx).await?;
		SemanticPoint::schedule(tx, &entry, &plan.collection).await?;
		if let Some(digest) = digest {
			let sql = Query::update()
				.table(Alias::new("semantic_points"))
				.value_expr(Alias::new("content_digest"), Expr::value(digest))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder);
			native::query(&sql).execute(&mut *tx).await?;
		}
	}
	SemanticHistory::append(
		tx,
		thread,
		None,
		plan.revision,
		"CONFIGURED",
		"Tenant embedding switched",
	)
	.await
}
