//! Duty Triggers other than the clock, the events they bring and the bookings
//! a Task makes for the next event of a trigger (#1011, ADR 0142).
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::duties::events::{DutyEvent, DutyTrigger, EventOutcome, TriggerKind};
use chrono::{DateTime, Utc};
use reinhardt::db::{backends::TransactionExecutor, orm::Model};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, LockBehavior, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use uuid::Uuid;

use crate::apps::workspaces::models::{DutyBookingRecord, DutyEventRecord, DutyTriggerRecord};
use crate::database::native::{self, Pool};
use crate::{Error, Result};

fn framework(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	Error::from(error.into())
}

fn trigger_of(row: DutyTriggerRecord) -> Result<DutyTrigger> {
	let kind = match (row.kind.as_str(), row.source_duty_id) {
		("webhook", None) => TriggerKind::Webhook,
		("duty_completed", Some(source_duty_id)) => TriggerKind::DutyCompleted { source_duty_id },
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
	})
}

pub(crate) async fn insert_trigger(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	workspace: Uuid,
	trigger: &DutyTrigger,
) -> Result<()> {
	let source = match &trigger.kind {
		TriggerKind::Webhook => None,
		TriggerKind::DutyCompleted { source_duty_id } => Some(*source_duty_id),
	};
	let row = DutyTriggerRecord::build()
		.id(trigger.id)
		.tenant(tenant.to_owned())
		.workspace_id(workspace)
		.duty_id(trigger.duty_id)
		.kind(trigger.kind.as_str().to_owned())
		.source_duty_id(source)
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

/// The Tenant and Workspace of a trigger named only by its id, as a webhook
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
	workspace: Uuid,
	event: &DutyEvent,
	run: Option<Uuid>,
) -> Result<()> {
	let row = DutyEventRecord::build()
		.id(event.id)
		.tenant(tenant.to_owned())
		.workspace_id(workspace)
		.duty_id(event.duty_id)
		.trigger_id(event.trigger_id)
		.received_at(event.received_at)
		.payload(reinhardt::db::orm::Json(event.payload.clone()))
		.outcome(event.outcome.as_str().to_owned())
		.reason(event.reason.clone())
		.task_id(event.task_id)
		.run_id(run)
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
	Ok(Some((event_of(row)?, kind.unwrap_or_default())))
}

/// Books the next event of `trigger` for the Task the Run works on.
pub(crate) async fn book(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	workspace: Uuid,
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
		.workspace_id(workspace)
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
					.from(Alias::new("runs"))
					.and_where(
						Expr::col((Alias::new("runs"), Alias::new("id")))
							.equals((Alias::new("duty_bookings"), Alias::new("run_id"))),
					)
					.and_where(
						Expr::col((Alias::new("runs"), Alias::new("phase"))).is_not_in([
							"COMPLETED",
							"FAILED",
							"CANCELLED",
						]),
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
/// its parent: the activation is the Run's Workspace event.
pub(crate) async fn wake(tx: &mut dyn TransactionExecutor, run: Uuid, event: Uuid) -> Result<()> {
	// The lock orders this producer as the Workspace wait triggers order
	// theirs (`aidash_workspace_task_activation`).
	native::query(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![
						Expr::value(format!("aidash:workspace-wait:{run}")).into(),
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

/// What a Workspace wait's activation names for a delivered event.
pub(crate) fn event_of_activation(reason: &str) -> Option<Uuid> {
	reason
		.strip_prefix("duty_event:")
		.and_then(|id| Uuid::parse_str(id).ok())
}
