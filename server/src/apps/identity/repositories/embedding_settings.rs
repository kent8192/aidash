use crate::{Error, Result, database::native};
use aidash_application::embedding::{EmbeddingRef, EmbeddingSettingsPort, default_embedding};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const TABLE: &str = "tenant_embedding_settings";

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "TenantEmbeddingSelection")]
pub struct Settings {
	pub active: EmbeddingRef,
	pub pending: Option<EmbeddingRef>,
	pub revision: i64,
}

impl Default for Settings {
	fn default() -> Self {
		Self {
			active: default_embedding(),
			pending: None,
			revision: 0,
		}
	}
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "TenantEmbeddingSettingsUpdate")]
pub struct Update {
	pub embedding: EmbeddingRef,
	pub expected_revision: i64,
	#[serde(default)]
	pub confirm: bool,
}

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "TenantReembeddingEstimate")]
pub struct Estimate {
	pub thread_indexes: i64,
	pub thread_entries: i64,
	/// Live Drive files selected for indexing, before passage extraction.
	pub drive_files: i64,
}

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "TenantEmbeddingSettingsResponse")]
pub struct Response {
	pub settings: Settings,
	pub embeddings: Vec<EmbeddingRef>,
	pub estimate: Option<Estimate>,
}

pub(crate) struct EmbeddingSettings<'a>(pub tokio::sync::Mutex<&'a mut dyn TransactionExecutor>);

#[async_trait::async_trait]
impl EmbeddingSettingsPort for EmbeddingSettings<'_> {
	async fn active_embedding(
		&self,
		tenant: &str,
	) -> aidash_application::Result<Option<EmbeddingRef>> {
		let mut tx = self.0.lock().await;
		Ok(Some(read(&mut **tx, tenant).await?.active))
	}
}

pub(crate) async fn read(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<Settings> {
	let sql = Query::select()
		.columns(["active", "pending", "revision"].map(Alias::new))
		.from(Alias::new(TABLE))
		.and_where(Expr::col("tenant").eq(tenant))
		.to_string(PostgresQueryBuilder);
	let Some(row) = native::query(&sql).fetch_optional(tx).await? else {
		return Ok(Settings::default());
	};
	let pending = row.try_get::<Option<Value>>("pending")?;
	Ok(Settings {
		active: serde_json::from_value(row.try_get::<Value>("active")?)?,
		pending: pending.map(serde_json::from_value).transpose()?,
		revision: row.try_get("revision")?,
	})
}

pub(crate) async fn embeddings(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<Vec<EmbeddingRef>> {
	let sql = Query::select()
		.columns(["id", "version"].map(Alias::new))
		.from(Alias::new("registry"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("kind").eq("embedding"))
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.order_by(Alias::new("version"), reinhardt::query::Order::Asc)
		.to_string(PostgresQueryBuilder);
	native::query(&sql)
		.fetch_all(tx)
		.await?
		.into_iter()
		.map(|row| {
			Ok(EmbeddingRef {
				id: row.try_get("id")?,
				version: row.try_get("version")?,
			})
		})
		.collect()
}

async fn count(tx: &mut dyn TransactionExecutor, table: &'static str, tenant: &str) -> Result<i64> {
	let mut query = Query::select();
	query
		.expr(Func::count(Expr::col("thread_id").into()))
		.from(Alias::new(table))
		.and_where(Expr::col("tenant").eq(tenant));
	if table == "semantic_entries" {
		query.and_where(Expr::col("deleted").eq(false));
	}
	Ok(native::query_scalar(&query.to_string(PostgresQueryBuilder))
		.scalar_one(tx)
		.await?)
}

pub(crate) async fn lock_tenant(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	lock: LockType,
) -> Result<()> {
	let sql = Query::select()
		.column(Alias::new("name"))
		.from(Alias::new("tenants"))
		.and_where(Expr::col("name").eq(tenant))
		.lock(lock)
		.to_string(PostgresQueryBuilder);
	native::query(&sql)
		.fetch_optional(tx)
		.await?
		.ok_or_else(|| Error::NotFound("tenant not found".into()))?;
	Ok(())
}

pub(crate) async fn drive_files(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<i64> {
	// Worker C creates the Drive tables. Until that migration is present,
	// this branch has no Drive inventory.
	let exists: bool = native::query_scalar(
		&Query::select()
			.expr(Expr::cust(
				"to_regclass('public.drive_entries') IS NOT NULL",
			))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await?;
	if !exists {
		return Ok(0);
	}
	let sql = Query::select()
		.expr(Func::count(Expr::col("path").into()))
		.from(Alias::new("drive_entries"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("indexed").eq(true))
		.and_where(Expr::col("removed_in_version").is_null())
		.to_string(PostgresQueryBuilder);
	Ok(native::query_scalar(&sql).scalar_one(tx).await?)
}

pub(crate) async fn change(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	update: Update,
) -> Result<Option<Estimate>> {
	lock_tenant(tx, tenant, LockType::Update).await?;
	let current = read(tx, tenant).await?;
	if current.revision != update.expected_revision {
		return Err(Error::Conflict(
			"embedding settings revision changed".into(),
		));
	}
	let entry = crate::apps::registry::models::Definition::read_of(
		tx,
		tenant,
		&update.embedding.id,
		&update.embedding.version,
	)
	.await?;
	if entry.kind != "embedding" {
		return Err(Error::Invalid(
			"choose a registered embedding definition".into(),
		));
	}
	let thread_indexes = count(tx, "semantic_indexes", tenant).await?;
	let drive_files = drive_files(tx, tenant).await?;
	if !update.confirm {
		return Ok(Some(Estimate {
			thread_indexes,
			thread_entries: count(tx, "semantic_entries", tenant).await?,
			drive_files,
		}));
	}
	if current.pending.as_ref() == Some(&update.embedding)
		|| current.active == update.embedding && current.pending.is_none()
	{
		return Ok(None);
	}

	let (active, pending) = if thread_indexes == 0 && drive_files == 0 {
		(update.embedding, None)
	} else {
		// Keep queries on the active generation until all replacement indexes
		// have been built and activated.
		let pending = (current.active != update.embedding)
			.then(|| serde_json::to_value(update.embedding))
			.transpose()?;
		(current.active, pending)
	};
	let sql = Query::insert()
		.into_table(Alias::new(TABLE))
		.columns(["tenant", "active", "pending", "revision", "next_attempt"].map(Alias::new))
		.values_panic([
			Expr::value(tenant),
			Expr::value(serde_json::to_value(active)?),
			Expr::value(pending),
			Expr::value(
				current.revision.checked_add(1).ok_or_else(|| {
					Error::Conflict("embedding settings revision exhausted".into())
				})?,
			),
		])
		.on_conflict(
			OnConflict::column(Alias::new("tenant"))
				.update_columns(["active", "pending", "revision", "next_attempt"].map(Alias::new))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	native::query(&sql).execute(tx).await?;
	Ok(None)
}

pub(crate) async fn defer(store: &crate::store::Store, tenant: &str, revision: i64) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let sql = Query::update()
		.table(Alias::new(TABLE))
		.value_expr(
			Alias::new("next_attempt"),
			Expr::value(chrono::Utc::now() + chrono::Duration::minutes(5)),
		)
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("revision").eq(revision))
		.to_string(PostgresQueryBuilder);
	native::query(&sql)
		.execute(store.pool.in_scope(&scope))
		.await?;
	Ok(())
}

pub(crate) async fn activate(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	revision: i64,
	target: &EmbeddingRef,
) -> Result<()> {
	let sql = Query::update()
		.table(Alias::new(TABLE))
		.value_expr(
			Alias::new("active"),
			Expr::value(serde_json::to_value(target)?),
		)
		.value_expr(Alias::new("pending"), Expr::null())
		.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("revision").eq(revision))
		.to_string(PostgresQueryBuilder);
	if native::query(&sql).execute(tx).await?.rows_affected() != 1 {
		return Err(Error::Conflict(
			"embedding settings revision changed".into(),
		));
	}
	Ok(())
}
