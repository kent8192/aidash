//! Deletion excludes immediately; body purge is a durable, idempotent PostgreSQL job.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::memory::{Retention, Unit};
use chrono::{Duration, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
mod dependencies;

pub(crate) async fn schedule(
	lease: &mut Lease<'_>,
	bank: uuid::Uuid,
	unit: &Unit,
	retention: &Retention,
) -> Result<()> {
	let now = Utc::now();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_purge_jobs"))
			.columns(
				[
					"unit_id",
					"bank_id",
					"revision",
					"purge_after",
					"backup_until",
					"state",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(unit.id))
					.expr(Expr::value(bank))
					.expr(Expr::value(unit.revision))
					.expr(Expr::value(
						now + Duration::seconds(i64::from(retention.purge_after_seconds)),
					))
					.expr(Expr::value(
						now + Duration::days(i64::from(retention.backup_days)),
					))
					.expr(Expr::value("pending"))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("unit_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

pub(crate) async fn history(
	lease: &mut Lease<'_>,
	unit: &Unit,
	retention: &Retention,
) -> Result<()> {
	let boundary = unit
		.revision
		.saturating_sub(retention.history_versions as i64);
	native::query(
		&Query::delete()
			.from_table(Alias::new("memory_history"))
			.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
			.and_where(
				reinhardt::query::Condition::any()
					.add(Expr::col("revision").lte(boundary))
					.add(Expr::col("updated_at").lt(Expr::value(
						Utc::now() - Duration::days(i64::from(retention.history_days)),
					))),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

pub(crate) async fn sweep(store: &Store) -> Result<usize> {
	let rows = native::query(
		&Query::select()
			.columns(["unit_id", "bank_id", "attempts"].map(Alias::new))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("state").eq("pending"))
			.and_where(Expr::col("purge_after").lte(Expr::value(Utc::now())))
			.and_where(Expr::col("next_attempt").lte(Expr::value(Utc::now())))
			.order_by(Alias::new("updated_at"), Order::Asc)
			.order_by(Alias::new("unit_id"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await?;
	let mut completed = 0;
	for row in rows {
		let id = row.try_get("unit_id")?;
		let attempts: i32 = row.try_get("attempts")?;
		let mut max_retries = None;
		let bank: uuid::Uuid = row.try_get("bank_id")?;
		let mut lease =
			Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
		let result = async {
			let workspace: uuid::Uuid = native::query_scalar(
				&Query::select()
					.column(Alias::new("workspace_id"))
					.from(Alias::new("memory_banks"))
					.and_where(Expr::col("id").eq(Expr::value(bank)))
					.and_where(Expr::col("home").eq(store.node_id.as_str()))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut **lease.tx())
			.await?
			.ok_or(Error::Forbidden)?;
			repository::lock_workspace(&mut lease, workspace, true).await?;
			let unit = units::load(&mut lease, id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			let job = native::query(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("memory_purge_jobs"))
					.and_where(Expr::col("unit_id").eq(Expr::value(id)))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **lease.tx())
			.await?;
			if job.try_get::<String>("state")? != "pending"
				|| job.try_get::<i32>("attempts")? != attempts
			{
				return Ok(false);
			}
			if !unit.deleted || job.try_get::<i64>("revision")? != unit.revision {
				return Err(Error::Conflict("purge deletion fence changed".into()));
			}
			let settings = super::bank_settings::get(&mut lease, &unit.bank)
				.await?
				.ok_or(Error::Forbidden)?;
			let policy =
				crate::semantic::native_memory::policy(&mut lease, &settings.provider).await?;
			max_retries = Some(policy.bounds.max_retries);
			native::query(
				&Query::delete()
					.from_table(Alias::new("memory_unit_retention"))
					.and_where(Expr::col("unit_id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			let affected_banks =
				dependencies::erase(&mut lease, &unit, bank, &policy.bounds).await?;
			// A model result can quote several units. Clearing the bank's result
			// bodies avoids claiming substring matching proves complete erasure.
			let operations = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("memory_model_operations"))
				.and_where(Expr::col("bank_id").is_in(affected_banks.into_iter().map(Expr::value)))
				.to_owned();
			native::query(
				&Query::update()
					.table(Alias::new("memory_model_attempts"))
					.value_expr(Alias::new("output"), Expr::value(None::<serde_json::Value>))
					.value_expr(Alias::new("state"), Expr::value("purged"))
					.and_where(Expr::col("operation_id").in_subquery(operations))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			// Existing cleanup rows enumerate every physical vector generation.
			let points = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("semantic_points"))
				.and_where(Expr::col("entry_id").eq(Expr::value(id)))
				.to_owned();
			native::query(
				&Query::delete()
					.from_table(Alias::new("semantic_vectors"))
					.and_where(Expr::col("id").in_subquery(points))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			native::query(
				&Query::update()
					.table(Alias::new("memory_purge_jobs"))
					.value_expr(Alias::new("state"), Expr::value("purged"))
					.value(Alias::new("attempts"), attempts.saturating_add(1))
					.value(Alias::new("last_error"), None::<String>)
					.value_expr(Alias::new("updated_at"), Expr::value(Utc::now()))
					.and_where(Expr::col("unit_id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			Ok(true)
		}
		.await;
		match lease.finish(result).await {
			Ok(true) => completed += 1,
			Ok(false) => {}
			Err(error) => {
				// A failing oldest deletion cannot stop later banks or indexing.
				// Store only redacted categories and the finite retry outcome.
				let code = match error {
					Error::Forbidden => "purge_authority_unavailable",
					Error::Conflict(_) => "purge_dependency_limit_or_fence",
					_ => "purge_storage_unavailable",
				};
				// Do not consume a retry before its policy could be loaded.
				let attempt = if max_retries.is_some() {
					attempts.saturating_add(1)
				} else {
					attempts
				};
				let state = if max_retries.is_none_or(|limit| (attempt as usize) < limit) {
					"pending"
				} else {
					"failed"
				};
				let mut tx = native::begin(&store.pool).await?;
				native::query(
					&Query::update()
						.table(Alias::new("memory_purge_jobs"))
						.value(Alias::new("attempts"), attempt)
						.value(Alias::new("state"), state)
						.value(Alias::new("last_error"), code)
						.value(Alias::new("updated_at"), Utc::now())
						.value(
							Alias::new("next_attempt"),
							Utc::now() + Duration::seconds(30),
						)
						.and_where(Expr::col("unit_id").eq(Expr::value(id)))
						.and_where(Expr::col("state").eq("pending"))
						.and_where(Expr::col("attempts").eq(attempts))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?;
				tx.commit().await?;
			}
		}
	}
	Ok(completed)
}
