//! Duty Triggers other than the clock, the events they bring and the bookings
//! a Task makes for the next event of a trigger (#1011, ADR 0142).
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::duties::events::{DutyEvent, DutyTrigger, EventOutcome, TriggerKind};
use chrono::{DateTime, Utc};
use reinhardt::db::{backends::TransactionExecutor, orm::Model};
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, IntoIden, IntoValue, LockBehavior, LockType, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

use crate::apps::workspaces::models::{DutyBookingRecord, DutyEventRecord, DutyTriggerRecord};
use crate::database::native::{self, Pool};
use crate::{Error, Result};

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	Error::from(error.into())
}

fn trigger_of(row: DutyTriggerRecord) -> Result<DutyTrigger> {
	let kind = match (row.kind.as_str(), row.source_duty_id, row.from_node) {
		("webhook", None, None) => TriggerKind::Webhook,
		("duty_completed", Some(source_duty_id), None) => {
			TriggerKind::DutyCompleted { source_duty_id }
		}
		("node_event", None, from_node) => TriggerKind::NodeEvent { from_node },
		_ => return Err(Error::Invalid("invalid Duty Trigger row".into())),
	};
	Ok(DutyTrigger {
		id: row.id,
		duty_id: row.duty_id,
		kind,
		created_at: row.created_at,
		created_by: row.created_by,
	})
}

fn event_of(row: DutyEventRecord) -> Result<DutyEvent> {
	Ok(DutyEvent {
		id: row.id,
		duty_id: row.duty_id,
		trigger_id: row.trigger_id,
		received_at: row.received_at,
		payload: row.payload.0,
		outcome: EventOutcome::parse(&row.outcome)?,
		reason: row.reason,
		task_id: row.task_id,
		router: row
			.router
			.map(|router| serde_json::from_value(router.0))
			.transpose()?,
	})
}

pub(crate) async fn insert_trigger(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	thread: Uuid,
	trigger: &DutyTrigger,
) -> Result<()> {
	let (source, from_node) = match &trigger.kind {
		TriggerKind::Webhook => (None, None),
		TriggerKind::DutyCompleted { source_duty_id } => (Some(*source_duty_id), None),
		TriggerKind::NodeEvent { from_node } => (None, from_node.clone()),
		TriggerKind::Watcher => {
			return Err(Error::Invalid(
				"a Watcher is part of its Duty, not a trigger row".into(),
			));
		}
	};
	let row = DutyTriggerRecord::build()
		.id(trigger.id)
		.tenant(tenant.to_owned())
		.thread_id(thread)
		.duty_id(trigger.duty_id)
		.kind(trigger.kind.as_str().to_owned())
		.source_duty_id(source)
		.from_node(from_node)
		.created_at(trigger.created_at)
		.created_by(trigger.created_by.clone())
		.finish();
	DutyTriggerRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

pub(crate) async fn triggers(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<Vec<DutyTrigger>> {
	DutyTriggerRecord::objects()
		.filter(DutyTriggerRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyTriggerRecord::field_duty_id().eq(duty))
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.map(trigger_of)
		.collect()
}

/// Deletes one trigger of `duty`; its bookings go with it. Whether it existed.
pub(crate) async fn delete_trigger(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	trigger: Uuid,
) -> Result<bool> {
	let rows = DutyTriggerRecord::objects()
		.filter(DutyTriggerRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyTriggerRecord::field_duty_id().eq(duty))
		.filter(DutyTriggerRecord::field_id().eq(trigger))
		.select_for_update()
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?;
	if rows.is_empty() {
		return Ok(false);
	}
	native::query(
		&Query::delete()
			.from_table(Alias::new("duty_bookings"))
			.and_where(Expr::col("trigger_id").eq(Expr::value(trigger)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	native::query(
		&Query::delete()
			.from_table(Alias::new("duty_triggers"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(Expr::value(trigger)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(true)
}

/// The Tenant and Thread of a trigger named only by its id, as a webhook
/// names it, read across Tenants.
pub(crate) async fn locate(pool: &Pool, trigger: Uuid) -> Result<Option<(String, DutyTrigger)>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::DutyTicks);
	let rows = DutyTriggerRecord::objects()
		.filter(DutyTriggerRecord::field_id().eq(trigger))
		.all_with_executor(&mut *pool.begin(&scope).await?)
		.await
		.map_err(framework)?;
	rows.into_iter()
		.next()
		.map(|row| {
			let tenant = row.tenant.clone();
			trigger_of(row).map(|trigger| (tenant, trigger))
		})
		.transpose()
}

/// The `duty_completed` triggers whose source is `duty`.
pub(crate) async fn triggers_on_completion(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<Vec<DutyTrigger>> {
	DutyTriggerRecord::objects()
		.filter(DutyTriggerRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyTriggerRecord::field_source_duty_id().eq(Some(duty)))
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.map(trigger_of)
		.collect()
}

pub(crate) async fn insert_event(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	thread: Uuid,
	event: &DutyEvent,
	run: Option<Uuid>,
) -> Result<()> {
	let row = DutyEventRecord::build()
		.id(event.id)
		.tenant(tenant.to_owned())
		.thread_id(thread)
		.duty_id(event.duty_id)
		.trigger_id(event.trigger_id)
		.received_at(event.received_at)
		.payload(reinhardt::db::orm::Json(event.payload.clone()))
		.outcome(event.outcome.as_str().to_owned())
		.reason(event.reason.clone())
		.task_id(event.task_id)
		.run_id(run)
		.router(
			event
				.router
				.as_ref()
				.map(|router| serde_json::to_value(router).map(reinhardt::db::orm::Json))
				.transpose()?,
		)
		.finish();
	DutyEventRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

/// The latest events of a Duty, newest first.
pub(crate) async fn events(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
	limit: u32,
) -> Result<Vec<DutyEvent>> {
	DutyEventRecord::objects()
		.filter(DutyEventRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyEventRecord::field_duty_id().eq(duty))
		.order_by(&["-received_at"])
		.limit(limit as usize)
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.into_iter()
		.map(event_of)
		.collect()
}

/// One delivered event, read for the Run it woke.
pub(crate) async fn event(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	id: Uuid,
) -> Result<Option<(DutyEvent, String)>> {
	let rows = DutyEventRecord::objects()
		.filter(DutyEventRecord::field_id().eq(id))
		.filter(DutyEventRecord::field_run_id().eq(Some(run)))
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?;
	let Some(row) = rows.into_iter().next() else {
		return Ok(None);
	};
	let kind: Option<String> = native::query_scalar(
		&Query::select()
			.column(Alias::new("kind"))
			.from(Alias::new("duty_triggers"))
			.and_where(Expr::col("id").eq(Expr::value(row.trigger_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	// A Watcher's events name their Duty as the trigger (#1011 D3).
	let watcher = row.trigger_id == row.duty_id;
	let kind = kind.unwrap_or_else(|| {
		if watcher {
			"watcher".into()
		} else {
			String::new()
		}
	});
	Ok(Some((event_of(row)?, kind)))
}

/// One event of the Tenant and the kind of trigger that brought it, for its
/// Router's decision. A Watcher's events name their Duty as the trigger; an
/// event whose trigger was removed since reads as a webhook's.
pub(crate) async fn find(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
) -> Result<Option<(DutyEvent, TriggerKind)>> {
	let Some(row) = DutyEventRecord::objects()
		.filter(DutyEventRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyEventRecord::field_id().eq(id))
		.select_for_update()
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?
		.pop()
	else {
		return Ok(None);
	};
	let kind = if row.trigger_id == row.duty_id {
		TriggerKind::Watcher
	} else {
		DutyTriggerRecord::objects()
			.filter(DutyTriggerRecord::field_tenant().eq(tenant.to_owned()))
			.filter(DutyTriggerRecord::field_id().eq(row.trigger_id))
			.all_with_executor(&mut *tx)
			.await
			.map_err(framework)?
			.pop()
			.map(trigger_of)
			.transpose()?
			.map_or(TriggerKind::Webhook, |trigger| trigger.kind)
	};
	Ok(Some((event_of(row)?, kind)))
}

/// Records what became of an event its Router decided about (#1011 D3): the
/// outcome and why, the decision, and the Task (and Run) it started or woke.
pub(crate) async fn conclude(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: Uuid,
	(outcome, reason): (EventOutcome, Option<String>),
	router: Option<&aidash_domain::duties::scripts::RouterAction>,
	(task, run): (Option<Uuid>, Option<Uuid>),
) -> Result<()> {
	let Some(mut row) = DutyEventRecord::objects()
		.filter(DutyEventRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyEventRecord::field_id().eq(id))
		.select_for_update()
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?
		.pop()
	else {
		return Ok(());
	};
	row.outcome = outcome.as_str().to_owned();
	row.reason = reason;
	row.task_id = task;
	row.run_id = run;
	row.router = router
		.map(|router| serde_json::to_value(router).map(reinhardt::db::orm::Json))
		.transpose()?;
	DutyEventRecord::objects()
		.save_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

/// Books the next event of `trigger` for the Task the Run works on.
pub(crate) async fn book(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	thread: Uuid,
	trigger: Uuid,
	task: Uuid,
	run: Uuid,
	now: DateTime<Utc>,
) -> Result<()> {
	let existing = DutyBookingRecord::objects()
		.filter(DutyBookingRecord::field_trigger_id().eq(trigger))
		.filter(DutyBookingRecord::field_run_id().eq(run))
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?;
	if !existing.is_empty() {
		return Ok(());
	}
	let row = DutyBookingRecord::build()
		.id(Uuid::new_v4())
		.tenant(tenant.to_owned())
		.thread_id(thread)
		.trigger_id(trigger)
		.task_id(task)
		.run_id(run)
		.created_at(now)
		.finish();
	DutyBookingRecord::objects()
		.insert_with_executor(tx, &row)
		.await
		.map_err(framework)?;
	Ok(())
}

/// Takes the oldest booking of `trigger` whose Run still waits, if any.
pub(crate) async fn take_booking(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	trigger: Uuid,
) -> Result<Option<(Uuid, Uuid)>> {
	let row = native::query(
		&Query::select()
			.columns(["id", "task_id", "run_id"].map(Alias::new))
			.from(Alias::new("duty_bookings"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("trigger_id").eq(Expr::value(trigger)))
			.and_where(Expr::exists(
				Query::select()
					.expr(Expr::value(1_i32))
					.from(Alias::new("task_executions"))
					.and_where(
						Expr::col((Alias::new("task_executions"), Alias::new("id")))
							.equals((Alias::new("duty_bookings"), Alias::new("run_id"))),
					)
					.and_where(
						Expr::col((Alias::new("task_executions"), Alias::new("phase")))
							.is_not_in(["COMPLETED", "FAILED", "CANCELLED"]),
					)
					.to_owned(),
			))
			.order_by(Alias::new("created_at"), Order::Asc)
			.limit(1)
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(row) = row else {
		return Ok(None);
	};
	let id: Uuid = row.try_get("id")?;
	native::query(
		&Query::delete()
			.from_table(Alias::new("duty_bookings"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(Some((row.try_get("task_id")?, row.try_get("run_id")?)))
}

/// Wakes the Run an event was delivered to, as a finished child Task wakes
/// its parent: the activation is the Run's Thread event.
pub(crate) async fn wake(tx: &mut dyn TransactionExecutor, run: Uuid, event: Uuid) -> Result<()> {
	// The lock orders this producer as the Thread wait triggers order
	// theirs (`aidash_thread_task_activation`).
	native::query(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![
						Expr::value(format!("aidash:thread-wait:{run}")).into(),
						Expr::value(71_500_644_i64).into(),
					],
				)],
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	native::query(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				"aidash_request_activation".into_iden(),
				vec![
					Expr::value(run).into(),
					Expr::value(format!("duty_event:{event}")).into(),
				],
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

/// What a Thread wait's activation names for a delivered event.
pub(crate) fn event_of_activation(reason: &str) -> Option<Uuid> {
	reason
		.strip_prefix("duty_event:")
		.and_then(|id| Uuid::parse_str(id).ok())
}

/// One webhook delivery as a trigger keeps it (#1011): the SHA-256 of its
/// body, which its signature covers, and the delivery id its sender named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Delivery {
	pub digest: String,
	pub id: Option<String>,
}

/// How long a trigger remembers the deliveries it accepted.
pub(crate) const DELIVERY_RETENTION_DAYS: i64 = 7;

/// The event an earlier delivery of the same body, or with the same delivery
/// id, recorded for `trigger`, if the trigger still remembers one.
pub(crate) async fn delivered(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	trigger: Uuid,
	delivery: &Delivery,
) -> Result<Option<DutyEvent>> {
	let mut same = Condition::any().add(Expr::col("digest").eq(delivery.digest.clone()));
	if let Some(id) = &delivery.id {
		same = same.add(Expr::col("delivery_id").eq(id.clone()));
	}
	let earlier: Option<(Uuid, Uuid, DateTime<Utc>)> = native::query_as(
		&Query::select()
			.columns(["event_id", "duty_id", "received_at"].map(Alias::new))
			.from(Alias::new("duty_deliveries"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("trigger_id").eq(Expr::value(trigger)))
			.cond_where(same)
			.order_by(Alias::new("received_at"), Order::Asc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["event_id", "duty_id", "received_at"])
	.fetch_optional(&mut *tx)
	.await?;
	let Some((event, duty, received_at)) = earlier else {
		return Ok(None);
	};
	let recorded = DutyEventRecord::objects()
		.filter(DutyEventRecord::field_tenant().eq(tenant.to_owned()))
		.filter(DutyEventRecord::field_id().eq(event))
		.all_with_executor(&mut *tx)
		.await
		.map_err(framework)?
		.pop();
	Ok(Some(match recorded {
		Some(row) => event_of(row)?,
		None => DutyEvent {
			id: event,
			duty_id: duty,
			trigger_id: trigger,
			received_at,
			payload: serde_json::Value::Null,
			outcome: EventOutcome::Skipped,
			reason: Some("a repeat of an earlier delivery".into()),
			task_id: None,
			router: None,
		},
	}))
}

/// Remembers a delivery with the event it recorded, in that event's
/// transaction, so a delivery that failed for now can be sent again.
pub(crate) async fn record_delivery(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	event: &DutyEvent,
	delivery: &Delivery,
) -> Result<()> {
	native::query(
		&Query::insert()
			.into_table(Alias::new("duty_deliveries"))
			.columns(
				[
					"id",
					"tenant",
					"duty_id",
					"trigger_id",
					"digest",
					"delivery_id",
					"event_id",
					"received_at",
				]
				.map(Alias::new),
			)
			.values_panic([
				IntoValue::into_value(Uuid::now_v7()),
				IntoValue::into_value(tenant.to_owned()),
				IntoValue::into_value(event.duty_id),
				IntoValue::into_value(event.trigger_id),
				IntoValue::into_value(delivery.digest.clone()),
				IntoValue::into_value(delivery.id.clone()),
				IntoValue::into_value(event.id),
				IntoValue::into_value(event.received_at),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}

/// Forgets, across Tenants, deliveries older than the retention, at most
/// `limit` per call. Returns how many went.
pub(crate) async fn forget_deliveries(
	pool: &Pool,
	before: DateTime<Utc>,
	limit: u64,
) -> Result<u64> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::Retention);
	let deleted = native::query(
		&Query::delete()
			.from_table(Alias::new("duty_deliveries"))
			.and_where(
				Expr::col("id").in_subquery(
					Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("duty_deliveries"))
						.and_where(Expr::col("received_at").lt(Expr::value(before)))
						.order_by(Alias::new("received_at"), Order::Asc)
						.limit(limit)
						.to_owned(),
				),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool.in_scope(&scope))
	.await?;
	Ok(deleted.rows_affected())
}

/// The condition of a booking whose Run has ended or is gone.
fn booking_ended() -> SimpleExpr {
	Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from(Alias::new("task_executions"))
			.and_where(
				Expr::col((Alias::new("task_executions"), Alias::new("id")))
					.equals((Alias::new("duty_bookings"), Alias::new("run_id"))),
			)
			.and_where(
				Expr::col((Alias::new("task_executions"), Alias::new("phase"))).is_not_in([
					"COMPLETED",
					"FAILED",
					"CANCELLED",
				]),
			)
			.to_owned(),
	)
	.not()
}

/// Drops, across Tenants, bookings whose Run ended (#1011): no event can be
/// delivered to them any more. At most `limit` per call; returns how many.
pub(crate) async fn forget_ended_bookings(pool: &Pool, limit: u64) -> Result<u64> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::Retention);
	let deleted = native::query(
		&Query::delete()
			.from_table(Alias::new("duty_bookings"))
			.and_where(
				Expr::col("id").in_subquery(
					Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("duty_bookings"))
						.and_where(booking_ended())
						.order_by(Alias::new("created_at"), Order::Asc)
						.limit(limit)
						.to_owned(),
				),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(pool.in_scope(&scope))
	.await?;
	Ok(deleted.rows_affected())
}

/// Drops every booking of a Duty's triggers (#1011): when a successor takes
/// the Duty over, no Run acting for the departed Requester gets its events.
pub(crate) async fn drop_bookings(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	duty: Uuid,
) -> Result<()> {
	native::query(
		&Query::delete()
			.from_table(Alias::new("duty_bookings"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(
				Expr::col("trigger_id").in_subquery(
					Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("duty_triggers"))
						.and_where(Expr::col("tenant").eq(tenant.to_owned()))
						.and_where(Expr::col("duty_id").eq(Expr::value(duty)))
						.to_owned(),
				),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	Ok(())
}
