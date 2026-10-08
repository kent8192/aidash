//! Disposable receiver quotations have a Home-pinned deadline and durable cleanup intent.
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{RunMetadata, semantic::remote::Receipt};
use chrono::{Duration, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct CleanupStatus {
	pub state: String,
	pub invalidated: bool,
	pub expires_at: chrono::DateTime<Utc>,
	pub attempts: i32,
	pub max_attempts: i32,
	pub next_attempt: chrono::DateTime<Utc>,
	pub last_error: Option<String>,
}
/// Only call after content-free Run management authorization, under its lease.
pub(crate) async fn status(
	tx: &mut native::Transaction,
	run: Uuid,
) -> Result<Option<CleanupStatus>> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receiver_caches"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **tx)
	.await?;
	row.map(|row| {
		Ok(CleanupStatus {
			state: row.try_get("state")?,
			invalidated: row.try_get("invalidated")?,
			expires_at: row.try_get("expires_at")?,
			attempts: row.try_get("attempts")?,
			max_attempts: row.try_get("max_attempts")?,
			next_attempt: row.try_get("next_attempt")?,
			last_error: row.try_get("last_error")?,
		})
	})
	.transpose()
}

pub(super) struct Gate {
	pub state: String,
	pub invalidated: bool,
	pub expires_at: chrono::DateTime<Utc>,
}
pub(super) async fn lock(tx: &mut native::Transaction, run: Uuid) -> Result<Option<Gate>> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receiver_caches"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **tx)
	.await?;
	row.map(|row| {
		Ok(Gate {
			state: row.try_get("state")?,
			invalidated: row.try_get("invalidated")?,
			expires_at: row.try_get("expires_at")?,
		})
	})
	.transpose()
}

pub(crate) async fn readable(tx: &mut native::Transaction, run: Uuid) -> Result<bool> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receiver_caches"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **tx)
	.await?;
	let Some(row) = row else { return Ok(true) };
	Ok(row.try_get::<String>("state")? == "pending"
		&& !row.try_get::<bool>("invalidated")?
		&& row.try_get::<chrono::DateTime<Utc>>("expires_at")? > Utc::now())
}

pub(crate) async fn available(store: &Store, run: Uuid) -> Result<bool> {
	let mut tx = native::begin(&store.control_pool).await?;
	let result = readable(&mut tx, run).await;
	tx.rollback().await?;
	result
}

pub(crate) async fn record(
	tx: &mut native::Transaction,
	run: Uuid,
	receipt: &Receipt,
) -> Result<()> {
	let Some(binding) = receipt.binding.native() else {
		return Ok(());
	};
	receipt.validate_native()?;
	let age = binding
		.banks
		.iter()
		.map(|bank| bank.cache_max_age_seconds)
		.min()
		.ok_or(Error::Forbidden)?;
	let attempts = binding
		.banks
		.iter()
		.map(|bank| bank.cache_max_attempts)
		.min()
		.ok_or(Error::Forbidden)?;
	let expires = receipt
		.retrieved_at
		.checked_add_signed(Duration::seconds(age as i64))
		.ok_or(Error::Forbidden)?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_receiver_caches"))
			.columns(
				[
					"run_id",
					"expires_at",
					"invalidated",
					"state",
					"attempts",
					"max_attempts",
					"next_attempt",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(run))
					.expr(Expr::value(expires))
					.expr(Expr::value(false))
					.expr(Expr::value("pending"))
					.expr(Expr::value(0_i32))
					.expr(Expr::value(attempts as i32))
					.expr(Expr::value(expires))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("run_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	let gate = lock(tx, run).await?.ok_or(Error::Forbidden)?;
	if gate.invalidated || gate.state != "pending" || gate.expires_at <= Utc::now() {
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::Invalidated,
		));
	}
	Ok(())
}

pub(super) async fn invalidate(store: &Store, run: Uuid) -> Result<()> {
	let mut tx = native::begin(&store.control_pool).await?;
	native::query(
		&Query::update()
			.table(Alias::new("memory_receiver_caches"))
			.value(Alias::new("invalidated"), true)
			.value(Alias::new("next_attempt"), Utc::now())
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("state").eq("pending"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await
}

pub(super) async fn complete(tx: &mut native::Transaction, run: Uuid) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("memory_receiver_caches"))
			.value(Alias::new("state"), "purged")
			.value(Alias::new("last_error"), None::<String>)
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}

pub(super) async fn purge(store: &Store, run: &RunMetadata) -> Result<()> {
	match super::remote_memory_reads::erase_receiver_body(store, run).await {
		Ok(()) => Ok(()),
		Err(error) => {
			let mut tx = native::begin(&store.control_pool).await?;
			if let Some(row) = native::query(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("memory_receiver_caches"))
					.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
					.and_where(Expr::col("state").eq("pending"))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
			{
				let attempts = row.try_get::<i32>("attempts")?.saturating_add(1);
				let maximum = row.try_get::<i32>("max_attempts")?;
				native::query(
					&Query::update()
						.table(Alias::new("memory_receiver_caches"))
						.value(Alias::new("attempts"), attempts.min(maximum))
						.value(Alias::new("invalidated"), true)
						.value(
							Alias::new("state"),
							if attempts >= maximum {
								"failed"
							} else {
								"pending"
							},
						)
						.value(Alias::new("last_error"), "receiver_storage_unavailable")
						.value(
							Alias::new("next_attempt"),
							Utc::now() + Duration::seconds(30),
						)
						.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?;
			}
			tx.commit().await?;
			Err(error)
		}
	}
}

pub(crate) async fn sweep(store: &Store) -> Result<()> {
	let rows: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_receiver_caches"))
			.and_where(Expr::col("state").eq("pending"))
			.and_where(Expr::col("next_attempt").lte(Expr::value(Utc::now())))
			.cond_where(
				Condition::any()
					.add(Expr::col("invalidated").eq(true))
					.add(Expr::col("expires_at").lte(Expr::value(Utc::now()))),
			)
			.order_by(Alias::new("next_attempt"), Order::Asc)
			.order_by(Alias::new("run_id"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&store.pool)
	.await?;
	for id in rows {
		let run = store.run(id).await?;
		if purge(store, &run.metadata()).await.is_err() {
			tracing::warn!(run = %id, "native receiver quotation cleanup postponed or failed");
		}
	}
	Ok(())
}
