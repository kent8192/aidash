//! Bounded Home maintenance applies the bank's pinned policy, never caller-selected caps.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{memory::*, registry::EntityRef};
use chrono::{Duration, Utc};
mod repairs;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, JoinType, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(crate) async fn sweep(store: &Store) -> Result<()> {
	// Filter busy banks before LIMIT: otherwise 32 active banks can permanently
	// hide every later maintenance candidate. Claiming all joined rows with
	// SKIP LOCKED also avoids waiting on settings readers or workspace writers.
	// Release the discovery locks before each independently bounded pass, which
	// reacquires and validates its own workspace/policy authority below.
	let mut discovery = native::begin(&store.pool).await?;
	let due = native::query(
		&Query::select()
			.columns(
				["bank_id", "provider_id", "provider_version", "revision"]
					.map(|column| ("memory_bank_settings", column)),
			)
			.from(Alias::new("memory_bank_settings"))
			.join(
				JoinType::InnerJoin,
				Alias::new("memory_banks"),
				Expr::col(("memory_bank_settings", "bank_id")).equals(("memory_banks", "id")),
			)
			.join(
				JoinType::InnerJoin,
				Alias::new("workspaces"),
				Expr::col(("memory_banks", "workspace_id")).equals(("workspaces", "id")),
			)
			.and_where(
				Expr::col(("memory_bank_settings", "next_maintenance"))
					.lte(Expr::value(Utc::now())),
			)
			.order_by(("memory_bank_settings", "next_maintenance"), Order::Asc)
			.order_by(("memory_bank_settings", "bank_id"), Order::Asc)
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *discovery)
	.await?;
	discovery.rollback().await?;
	for item in due {
		let id: uuid::Uuid = item.try_get("bank_id")?;
		let provider = EntityRef {
			id: item.try_get("provider_id")?,
			version: item.try_get("provider_version")?,
		};
		let mut lease =
			Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?;
		let result = async {
			let row = native::query(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("memory_banks"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **lease.tx())
			.await?;
			let bank = Bank {
				home: row.try_get("home")?,
				tenant: row.try_get("tenant")?,
				workspace: row.try_get("workspace_id")?,
				participant: row.try_get("participant_id")?,
			};
			if bank.home != store.node_id {
				return Err(Error::Forbidden);
			}
			// An indexer can retain a workspace reader while opening another
			// current-origin reader. Queuing a writer behind it blocks that reader
			// and creates a lock cycle across the indexer's independent scopes.
			// Discovery skips busy banks before its page limit; recheck races here.
			if native::query(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("workspaces"))
					.and_where(Expr::col("id").eq(Expr::value(bank.workspace)))
					.lock(LockType::Update)
					.lock_behavior(LockBehavior::SkipLocked)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **lease.tx())
			.await?
			.is_none()
			{
				return Ok(());
			}
			let current = super::bank_settings::get(&mut lease, &bank)
				.await?
				.ok_or(Error::Forbidden)?;
			if current.provider != provider {
				return Ok(());
			}
			let policy = crate::semantic::native_memory::policy(&mut lease, &provider).await?;
			let now = Utc::now();
			let retention = &policy.retention;
			// Staleness also retires derived rows when no viable automatic repair
			// remains. Preserve queued refreshes, but never let hidden rows without
			// work consume live capacity forever when age-based expiry is disabled.
			let mut retired = reinhardt::query::Condition::any().add(Expr::col("stale").eq(true));
			if let Some(age) = retention.unit_max_age_days {
				retired = retired.add(
					Expr::col("learned_at").lte(Expr::value(now - Duration::days(i64::from(age)))),
				);
			}
			{
				let expired = native::query(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("memory_units"))
						.and_where(Expr::col("bank_id").eq(Expr::value(id)))
						.and_where(Expr::col("deleted").eq(false))
						.cond_where(retired)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(policy.bounds.max_units as u64 + 1)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **lease.tx())
				.await?;
				let jobs = repairs::pending(&mut lease, id, &provider, &policy).await?;
				let mut changes = Vec::new();
				for item in expired {
					let unit = units::load(&mut lease, item.try_get("id")?, false)
						.await?
						.ok_or(Error::Forbidden)?;
					let aged = retention
						.unit_max_age_days
						.is_some_and(|age| unit.learned_at <= now - Duration::days(i64::from(age)));
					if !aged && repairs::viable(&mut lease, &unit, &policy, &jobs).await? {
						continue;
					}
					changes.push(Change::Delete {
						id: unit.id,
						expected_revision: unit.revision,
					});
					if changes.len() >= retention.purge_batch.min(policy.bounds.max_candidates) {
						break;
					}
				}
				if !changes.is_empty() {
					repository::mutate(
						&mut lease,
						&Mutation {
							operation_id: uuid::Uuid::now_v7(),
							provider: provider.clone(),
							bank: bank.clone(),
							changes,
						},
						&policy.bounds,
					)
					.await?;
				}
			}
			native::query(
				&Query::delete()
					.from_table(Alias::new("memory_history"))
					.and_where(Expr::col("bank_id").eq(Expr::value(id)))
					.cond_where(
						reinhardt::query::Condition::any()
							.add(Expr::col("updated_at").lt(Expr::value(
								now - Duration::days(i64::from(retention.history_days)),
							)))
							.add(Expr::exists(
								Query::select()
									.expr(Expr::value(1))
									.from_as(Alias::new("memory_units"), Alias::new("unit"))
									.and_where(
										Expr::col(("unit", "id"))
											.equals(("memory_history", "unit_id")),
									)
									.and_where(
										Expr::col(("memory_history", "revision")).lte(
											Expr::col(("unit", "revision"))
												.sub(retention.history_versions as i64),
										),
									)
									.to_owned(),
							)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			// Expired candidates remain content-free dispositions, preserving the
			// review CAS and preventing a deterministic proposal ID from reopening.
			native::query(
				&Query::update()
					.table(Alias::new("memory_candidates"))
					.value_expr(Alias::new("text"), Expr::value(""))
					.value_expr(
						Alias::new("occurred_start"),
						Expr::value(None::<chrono::DateTime<Utc>>),
					)
					.value_expr(
						Alias::new("occurred_end"),
						Expr::value(None::<chrono::DateTime<Utc>>),
					)
					.value_expr(
						Alias::new("mental_model"),
						Expr::value(None::<serde_json::Value>),
					)
					.value_expr(Alias::new("entities"), Expr::value(serde_json::json!([])))
					.value_expr(Alias::new("evidence"), Expr::value(serde_json::json!([])))
					.value_expr(Alias::new("links"), Expr::value(serde_json::json!([])))
					.value_expr(Alias::new("state"), Expr::value("invalidated"))
					.value_expr(Alias::new("revision"), Expr::col("revision").add(1_i64))
					.value_expr(Alias::new("updated_at"), Expr::value(now))
					.and_where(Expr::col("bank_id").eq(Expr::value(id)))
					.and_where(Expr::col("created_at").lt(Expr::value(
						now - Duration::days(i64::from(retention.candidate_days)),
					)))
					.and_where(
						Expr::col("text")
							.ne("")
							.or(Expr::col("occurred_start").is_not_null())
							.or(Expr::col("occurred_end").is_not_null()),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			let expired_operations = Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("memory_model_operations"))
				.and_where(Expr::col("bank_id").eq(Expr::value(id)))
				.and_where(Expr::col("created_at").lt(Expr::value(
					now - Duration::days(i64::from(retention.model_result_days)),
				)))
				.to_owned();
			native::query(
				&Query::update()
					.table(Alias::new("memory_model_attempts"))
					.value_expr(Alias::new("output"), Expr::value(None::<serde_json::Value>))
					.value_expr(Alias::new("state"), Expr::value("purged"))
					.and_where(Expr::col("operation_id").in_subquery(expired_operations))
					.and_where(Expr::col("state").ne("purged"))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			native::query(
				&Query::update()
					.table(Alias::new("memory_bank_settings"))
					.value_expr(
						Alias::new("next_maintenance"),
						Expr::value(now + Duration::seconds(30)),
					)
					.and_where(Expr::col("bank_id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
			Ok(())
		}
		.await;
		if lease.finish(result).await.is_err() {
			// A broken bank cannot monopolize the oldest due page. Advance
			// only the exact observed provider/revision after rolling back.
			let mut tx = native::begin(&store.pool).await?;
			// A reader can arrive after the failed pass releases its workspace.
			// Never queue a settings writer behind it while postponing a retry.
			if native::query(
				&Query::select()
					.column(Alias::new("bank_id"))
					.from(Alias::new("memory_bank_settings"))
					.and_where(Expr::col("bank_id").eq(Expr::value(id)))
					.and_where(
						Expr::col("revision").eq(Expr::value(item.try_get::<i64>("revision")?)),
					)
					.and_where(Expr::col("provider_id").eq(provider.id.as_str()))
					.and_where(Expr::col("provider_version").eq(provider.version.as_str()))
					.lock(LockType::Update)
					.lock_behavior(LockBehavior::SkipLocked)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
			.is_none()
			{
				tx.rollback().await?;
				continue;
			}

			native::query(
				&Query::update()
					.table(Alias::new("memory_bank_settings"))
					.value(
						Alias::new("next_maintenance"),
						Utc::now() + Duration::seconds(30),
					)
					.and_where(Expr::col("bank_id").eq(Expr::value(id)))
					.and_where(
						Expr::col("revision").eq(Expr::value(item.try_get::<i64>("revision")?)),
					)
					.and_where(Expr::col("provider_id").eq(provider.id.as_str()))
					.and_where(Expr::col("provider_version").eq(provider.version.as_str()))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			tx.commit().await?;
			tracing::warn!(bank = %id, "native memory retention postponed after a failed bounded maintenance pass");
		}
	}
	Ok(())
}
