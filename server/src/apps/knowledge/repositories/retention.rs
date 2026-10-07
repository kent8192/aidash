//! Bounded Home maintenance applies the bank's pinned policy, never caller-selected caps.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{memory::*, registry::EntityRef};
use chrono::{Duration, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(crate) async fn sweep(store: &Store) -> Result<()> {
	let due = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_bank_settings"))
			.and_where(Expr::col("next_maintenance").lte(Expr::value(Utc::now())))
			.order_by(Alias::new("next_maintenance"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await?;
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
			repository::lock_workspace(&mut lease, bank.workspace, true).await?;
			let current = super::bank_settings::get(&mut lease, &bank)
				.await?
				.ok_or(Error::Forbidden)?;
			if current.provider != provider {
				return Ok(());
			}
			let policy = crate::semantic::native_memory::policy(&mut lease, &provider).await?;
			let now = Utc::now();
			let retention = &policy.retention;
			// Ordinary stale units have no automatic repair job. Retire them even
			// when age-based expiry is disabled; derived units remain repairable.
			let mut retired = reinhardt::query::Condition::any().add(
				reinhardt::query::Condition::all()
					.add(Expr::col("stale").eq(true))
					.add(Expr::col("kind").is_in(["world", "experience"])),
			);
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
						.limit(retention.purge_batch.min(policy.bounds.max_candidates) as u64)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **lease.tx())
				.await?;
				let mut changes = Vec::new();
				for item in expired {
					let unit = units::load(&mut lease, item.try_get("id")?, false)
						.await?
						.ok_or(Error::Forbidden)?;
					changes.push(Change::Delete {
						id: unit.id,
						expected_revision: unit.revision,
					});
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
					.and_where(Expr::col("updated_at").lt(Expr::value(
						now - Duration::days(i64::from(retention.history_days)),
					)))
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
