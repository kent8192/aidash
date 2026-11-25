//! Price and post observed traffic in the transaction that advances its byte checkpoint.
use aidash_application::egress::Direction;
use aidash_domain::egress::{
	ScopeId,
	price::{EgressPrices, cost_increase},
};
use aidash_domain::entities::{ControlSource, RunControlAction};
use chrono::{DateTime, NaiveDate, Utc};
use reinhardt::db::orm::Model;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;

use crate::apps::execution::capabilities::models::egress::{EgressUsageDaily, ScopeConnection};
use crate::apps::identity::repositories::budgets;
use crate::database::native::{self, Pool, Transaction};
use crate::{Error, Result, store::Store};

const BUDGET_PAUSE: &str = "Egress Budget Hard Limit reached; raise the limit, then resume the Run";

pub(super) async fn paused(tx: &mut Transaction, scope: &ScopeId, run: Uuid) -> Result<bool> {
	Ok(native::query_scalar::<Uuid>(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.and_where(Expr::col("tenant").eq(Expr::value(scope.tenant.clone())))
			.and_where(Expr::col("control").eq("PAUSED"))
			.and_where(Expr::col("error").eq(BUDGET_PAUSE))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(tx.as_mut())
	.await?
	.is_some())
}

pub(super) struct Traffic {
	pub prices: EgressPrices,
	pub direction: Direction,
	pub previous: i64,
	pub cumulative: i64,
	pub day: NaiveDate,
}

impl Traffic {
	pub async fn post(
		&self,
		store: &Store,
		tx: &mut Transaction,
		scope: &ScopeId,
		run: Option<Uuid>,
	) -> Result<bool> {
		let bytes = self.cumulative.saturating_sub(self.previous);
		if bytes <= 0 {
			return match run {
				Some(run) => paused(tx, scope, run).await,
				None => Ok(false),
			};
		}
		let (sent, received, price) = match self.direction {
			Direction::Sent => (bytes, 0, self.prices.sent),
			Direction::Received => (0, bytes, self.prices.received),
		};
		let cost = cost_increase(price, self.previous, self.cumulative);
		let day = self.day.to_string();
		// The Tenant/day row survives Node and Scratch Space purge. Its update is
		// serialized with concurrent reports by PostgreSQL's row lock.
		native::query(
			&Query::insert()
				.into_table(Alias::new(EgressUsageDaily::table_name()))
				.columns(
					[
						"id",
						"tenant",
						"day",
						"sent_bytes",
						"received_bytes",
						"cost_nanocredits",
					]
					.map(Alias::new),
				)
				.values_panic([
					Uuid::new_v4().into_value(),
					scope.tenant.clone().into_value(),
					day.clone().into_value(),
					0_i64.into_value(),
					0_i64.into_value(),
					0_i64.into_value(),
				])
				.on_conflict(
					OnConflict::columns(["tenant", "day"].map(Alias::new))
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(tx.as_mut())
		.await?;
		native::query(
			&Query::update()
				.table(Alias::new(EgressUsageDaily::table_name()))
				.value_expr(
					Alias::new("sent_bytes"),
					Expr::col("sent_bytes").add(Expr::value(sent)),
				)
				.value_expr(
					Alias::new("received_bytes"),
					Expr::col("received_bytes").add(Expr::value(received)),
				)
				.value_expr(
					Alias::new("cost_nanocredits"),
					Expr::col("cost_nanocredits").add(Expr::value(cost)),
				)
				.and_where(Expr::col("tenant").eq(Expr::value(scope.tenant.clone())))
				.and_where(Expr::col("day").eq(Expr::value(day)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(tx.as_mut())
		.await?;
		let Some(run) = run else {
			return Ok(false);
		};
		let active = native::query_scalar::<String>(
			&Query::select()
				.column(Alias::new("control"))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col("id").eq(Expr::value(run)))
				.and_where(Expr::col("tenant").eq(Expr::value(scope.tenant.clone())))
				.and_where(
					Expr::col("phase")
						.is_not_in(["COMPLETED", "FAILED", "CANCELLED"].map(Expr::value)),
				)
				.lock(LockType::NoKeyUpdate)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(tx.as_mut())
		.await?
		.is_some_and(|control| control == "ACTIVE");
		let Some(charge) = budgets::charge_of(tx.as_mut(), run).await? else {
			return Ok(false);
		};
		if charge.tenant != scope.tenant {
			return Err(Error::Forbidden);
		}
		let now = Utc::now();
		let spent_at = self
			.day
			.and_hms_opt(0, 0, 0)
			.expect("UTC midnight")
			.and_utc();
		if cost > 0 {
			budgets::record_charge(tx.as_mut(), &charge, Some(cost), spent_at).await?;
		}
		let (gate, _) = budgets::gate_for(tx.as_mut(), &charge, now).await?;
		let Err(Error::BudgetExhausted { .. }) = budgets::admit(gate) else {
			return Ok(false);
		};
		if active {
			let paused = store
				.control_one_in_as(
					tx.as_mut(),
					run,
					RunControlAction::Pause,
					ControlSource::System,
				)
				.await?;
			native::query(
				&Query::update()
					.table(Alias::new("task_executions"))
					.value(Alias::new("error"), BUDGET_PAUSE)
					.and_where(Expr::col("id").eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(tx.as_mut())
			.await?;

			store
				.run_event(
					tx,
					run,
					paused
						.is_local(store.deployment.as_str())
						.then_some(paused.thread_id),
					"execution.budget_exhausted",
					serde_json::json!({
						"run_id": run, "task_id": paused.task_id,
						"message": BUDGET_PAUSE, "source": "egress",
					}),
				)
				.await?;
		}
		Ok(true)
	}

	pub fn observe(&self, run: Option<Uuid>, now: DateTime<Utc>) {
		let bytes = self.cumulative.saturating_sub(self.previous);
		if let Some(run) = run
			&& let Direction::Sent = self.direction
			&& bytes > 0
		{
			metrics::counter!("aidash_egress_observed_sent_bytes_total").increment(bytes as u64);
			tracing::info!(
				%run, utc_hour = %now.format("%Y-%m-%dT%H:00Z"), sent_bytes = bytes,
				"egress_run_hour_sample"
			);
		}
	}
}

/// CONNECT exposes a host and total bytes, so a transfer here is a completed
/// Host connection. Match within a Run over the last two weeks.
async fn similar_transfer(
	tx: &mut Transaction,
	row: &ScopeConnection,
	run: Uuid,
	now: DateTime<Utc>,
) -> Result<bool> {
	// The two-sided comparison is integral and cannot overflow.
	let tolerance = row.sent_bytes / 100;
	let previous = ScopeConnection::objects()
		.filter(ScopeConnection::field_tenant().eq(row.tenant.clone()))
		.filter(ScopeConnection::field_run_id().eq(Some(run)))
		.filter(ScopeConnection::field_host().eq(row.host.clone()))
		.filter(ScopeConnection::field_id().ne(row.id))
		.filter(ScopeConnection::field_outcome().eq("completed".to_owned()))
		.filter(ScopeConnection::field_created_at().gte(now - chrono::Duration::days(14)))
		.filter(ScopeConnection::field_sent_bytes().gte(row.sent_bytes - tolerance))
		.filter(ScopeConnection::field_sent_bytes().lte(row.sent_bytes.saturating_add(tolerance)))
		.limit(1)
		.all_with_executor(&mut **tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	Ok(!previous.is_empty())
}

pub(super) async fn observe_transfer(pool: &Pool, row: &ScopeConnection, now: DateTime<Utc>) {
	let Some(run) = row.run_id else {
		return;
	};
	if row.sent_bytes == 0 || row.outcome != "completed" {
		return;
	}
	metrics::histogram!("aidash_egress_observed_transfer_sent_bytes").record(row.sent_bytes as f64);
	let lookup = async {
		let mut tx = super::begin_timed(pool, &row.tenant).await?;
		let repeat = similar_transfer(&mut tx, row, run, now).await?;
		tx.commit().await?;
		Ok::<_, Error>(repeat)
	};
	// Observe after committing accounting, with a bounded read.
	match tokio::time::timeout(std::time::Duration::from_secs(1), lookup).await {
		Ok(Ok(true)) => {
			metrics::counter!("aidash_egress_observed_similar_transfers_total").increment(1);
			tracing::info!(%run, connection = %row.id, sent_bytes = row.sent_bytes, "egress_similar_sent_transfer");
		}
		Ok(Ok(false)) => {}
		_ => metrics::counter!("aidash_egress_observation_failures_total").increment(1),
	}
}
