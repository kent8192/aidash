//! Lease and acknowledge durable events through the native ORM.
use super::{Event, Inbox};
use crate::Result;
use crate::database::native::Pool;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use chrono::{DateTime, Duration, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::query::FieldAssignment;
use reinhardt::db::orm::{AtomicTransaction, Model};
use reinhardt::query::{
	Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

/// Publishing claims and acknowledges every Tenant's events.
const OUTBOX: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::OutboxDispatch);

async fn database_time(tx: &mut AtomicTransaction) -> Result<DateTime<Utc>> {
	let (sql, values) = Query::select()
		.expr_as(Expr::current_timestamp(), Alias::new("now"))
		.build(PostgresQueryBuilder);
	let row = TransactionExecutor::fetch_one(tx, &sql, convert_values(values)).await?;
	Ok(row.get("now").map_err(FrameworkError::from)?)
}

impl Event {
	/// Commit the retry deadline before publishing so concurrent publishers take
	/// disjoint batches. PostgreSQL's clock controls leases across process hosts.
	pub(crate) async fn claim_outbox(pool: &Pool) -> Result<Vec<Self>> {
		pool.atomic(&OUTBOX, async |tx| {
			let now = database_time(tx).await?;
			let mut events = Self::objects()
				.filter(Self::field_published_at().is_null())
				.filter(Self::field_next_attempt_at().lte(now))
				.order_by(&["next_attempt_at", "sequence"])
				.limit(100)
				.select_for_update()
				.skip_locked()
				.all_with_executor(tx)
				.await
				.map_err(FrameworkError::from)?;
			if !events.is_empty() {
				let retry_at = now + Duration::seconds(30);
				Self::objects()
					.filter(Self::field_sequence().is_in(events.iter().map(|event| event.sequence)))
					.update_fields_with_conn(tx, [(Self::field_next_attempt_at(), retry_at)])
					.await?;
				for event in &mut events {
					event.next_attempt_at = retry_at;
				}
			}
			Ok(events)
		})
		.await
	}

	/// A delayed attempt must not overwrite the outcome of a newer claim.
	pub(crate) async fn finish_publication(
		&self,
		pool: &Pool,
		error: Option<String>,
	) -> Result<bool> {
		pool.atomic(&OUTBOX, async |tx| {
			let now = database_time(tx).await?;
			let assignments: Vec<FieldAssignment> = match error {
				None => vec![
					(Self::field_published_at(), Some(now)).into(),
					(Self::field_publish_error(), None::<String>).into(),
				],
				Some(error) => vec![
					(Self::field_publish_error(), Some(error)).into(),
					(Self::field_next_attempt_at(), now + Duration::seconds(30)).into(),
				],
			};
			let changed = Self::objects()
				.filter(Self::field_id().eq(self.id))
				.filter(Self::field_published_at().is_null())
				.filter(Self::field_next_attempt_at().eq(self.next_attempt_at))
				.update_fields_with_conn(tx, assignments)
				.await?;
			Ok(changed == 1)
		})
		.await
	}
}

impl Inbox {
	/// Native ORM creation has no insert-or-ignore result. Use the Query builder
	/// for one atomic conflict-aware insert, preserving the affected-row count.
	pub(crate) async fn receive(pool: &Pool, event: Uuid) -> Result<bool> {
		// Received event ids are Deployment-wide.
		pool.atomic(&TenancyScope::NoTenant, async |tx| {
			let (sql, values) = Query::insert()
				.into_table(Alias::new(Self::table_name()))
				.columns([Alias::new("event_id")])
				.values_panic([IntoValue::into_value(event)])
				.on_conflict(OnConflict::columns(["event_id"]).do_nothing().to_owned())
				.build(PostgresQueryBuilder);
			Ok(
				TransactionExecutor::execute(tx, &sql, convert_values(values))
					.await?
					.rows_affected == 1,
			)
		})
		.await
	}
}

#[cfg(test)]
#[path = "../tests/outbox_records.rs"]
mod tests;

use reinhardt::query::IntoValue;
