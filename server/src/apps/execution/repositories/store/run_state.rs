use super::*;
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use chrono::{DateTime, Utc};
use reinhardt::query::{Alias, Condition, Expr, PostgresQueryBuilder, Query};
use std::time::Duration;
use tokio::time::Instant;
fn a(name: &str) -> Alias {
	Alias::new(name)
}

pub(crate) type RecoveryCursor = Option<(DateTime<Utc>, Uuid)>;
#[derive(Default)]
pub(crate) struct RecoveryCursors {
	pub execution: tokio::sync::Mutex<RecoveryCursor>,
	pub delivery: tokio::sync::Mutex<RecoveryCursor>,
	pub outage: Outage,
}

/// A streak of connection failures observed by this process (ADR 0123).
#[derive(Default)]
pub(crate) struct Outage(std::sync::Mutex<Option<FailureStreak>>);

struct FailureStreak {
	first: Instant,
	last: Instant,
}

impl Outage {
	/// A gap of at least one lease period lets a later failure start a new hold.
	pub fn note(&self, error: &crate::Error, lease_seconds: i32) {
		if !error.is_database_unreachable() {
			return;
		}
		let mut streak = self
			.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		let now = Instant::now();
		let period = Duration::from_secs(lease_seconds.max(0) as u64);
		let first = streak
			.as_ref()
			.filter(|streak| now.duration_since(streak.last) < period)
			.map_or(now, |streak| streak.first);
		*streak = Some(FailureStreak { first, last: now });
	}
	/// A streak holds recoveries for at most one lease period from its first failure.
	pub fn holds_recoveries(&self, lease_seconds: i32) -> bool {
		self.0
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner)
			.as_ref()
			.is_some_and(|streak| {
				streak.first.elapsed() < Duration::from_secs(lease_seconds.max(0) as u64)
			})
	}
}

pub(crate) struct FailureDelivery {
	pub metadata: RunMetadata,
	pub target: FailureTarget,
}

/// Cancelled Runs can settle even when no worker covers their requirements.
pub(crate) fn run_unblocked() -> Condition {
	Condition::all().add(Expr::col(a("control")).eq(Expr::value("CANCELLED")))
}

impl Store {
	pub(crate) async fn claim_failure_delivery(
		&self,
		worker: Uuid,
		seconds: i32,
	) -> Result<Option<FailureDelivery>> {
		let scope = &TenancyScope::Deployment(DeploymentScopeReason::ActivationClaim);
		let mut tx = self.pool.begin(scope).await?;
		let now: DateTime<Utc> = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("CURRENT_TIMESTAMP"))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *tx)
		.await?;
		let mut cursor = self.recovery_cursors.delivery.lock().await;
		let mut query = Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(a("task_executions"))
			.and_where(Expr::col(a("phase")).eq(reinhardt::query::Expr::value("WAITING")))
			.and_where(Expr::col(a("control")).ne(reinhardt::query::Expr::value("PAUSED")))
			.and_where(Expr::col(a("revision")).lt(reinhardt::query::Expr::value(i64::MAX - 2)))
			.and_where(Expr::cust(
				"pending -> 'data' ->> 'reason' = 'failure_delivery' AND (lease_until IS NULL OR lease_until <= CURRENT_TIMESTAMP)",
			))
			.order_by(a("updated_at"), reinhardt::query::Order::Asc)
			.order_by(a("id"), reinhardt::query::Order::Asc)
			.limit(128)
			.lock(
				reinhardt::query::LockType::Update).lock_behavior(reinhardt::query::LockBehavior::SkipLocked,
			)
			.to_owned();
		if let Some((at, id)) = *cursor {
			query.and_where(
				reinhardt::query::Condition::any()
					.add(Expr::col(a("updated_at")).gt(reinhardt::query::Expr::value(at)))
					.add(
						reinhardt::query::Condition::all()
							.add(Expr::col(a("updated_at")).eq(reinhardt::query::Expr::value(at)))
							.add(Expr::col(a("id")).gt(reinhardt::query::Expr::value(id))),
					),
			);
		}
		let rows: Vec<RawRun> =
			aidash_server::database::query_as(&query.to_string(PostgresQueryBuilder))
				.fetch_all(&mut *tx)
				.await?;
		let more = rows.len() == 128;
		for raw in rows {
			*cursor = Some((raw.updated_at, raw.id));
			let Ok((RunState::Waiting(wait), _)) =
				crate::domain::run_state::decode(raw.phase, raw.pending.clone())
			else {
				continue;
			};
			let WaitingState::FailureDelivery {
				target, wake_at, ..
			} = *wait
			else {
				continue;
			};
			if wake_at > now && raw.control != RunControl::Cancelled {
				continue;
			}
			// A saved disposition precedes later control changes. Cancellation
			// stops new execution; it cannot replace an undelivered failure.
			let metadata: Option<RunMetadata> = aidash_server::database::query_as(
				&Query::update()
					.table(a("task_executions"))
					.value(a("lease_owner"), worker)
					.value_expr(a("lease_until"), crate::database::lease_deadline(seconds))
					.value_expr(
						a("revision"),
						Expr::col(a("revision")).add(reinhardt::query::Expr::value(1)),
					)
					.value(a("ledger_worker_ready"), true)
					.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(raw.id)))
					.and_where(
						Expr::col(a("revision")).eq(reinhardt::query::Expr::value(raw.revision)),
					)
					.and_where(Expr::cust(
						"set_config('aidash.input_ledger_worker','true',true)='true'",
					))
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?;
			if let Some(metadata) = metadata {
				tx.commit().await?;
				return Ok(Some(FailureDelivery { metadata, target }));
			}
		}
		if !more {
			*cursor = None;
		}
		tx.commit().await?;
		Ok(None)
	}
	pub(crate) async fn finish_failure_delivery(
		&self,
		scope: &TenancyScope,
		delivery: &FailureDelivery,
		worker: Uuid,
		result: Result<TaskStatus>,
	) -> Result<()> {
		let (state, error, kind) = match result {
			Ok(status) => {
				let (state, kind) = match status {
					TaskStatus::Completed => {
						(RunState::Completed(TerminalState {}), "execution.completed")
					}
					TaskStatus::Failed => (RunState::Failed(TerminalState {}), "execution.failed"),
					TaskStatus::Cancelled | TaskStatus::Abandoned => {
						(RunState::Cancelled(TerminalState {}), "execution.cancelled")
					}
					TaskStatus::Open
					| TaskStatus::Claimed
					| TaskStatus::Running
					| TaskStatus::Blocked => {
						return Err(Error::Invalid(
							"terminal delivery returned an executable task".into(),
						));
					}
				};
				let error = if status == TaskStatus::Failed {
					delivery.target.reason().map(str::to_owned)
				} else {
					None
				};
				(state, error, kind)
			}
			Err(error) => {
				let text: String = error.to_string().chars().take(512).collect();
				(
					RunState::Waiting(Box::new(WaitingState::FailureDelivery {
						target: delivery.target,
						wake_at: Utc::now() + chrono::Duration::seconds(5),
						last_delivery_error: Some(text.clone()),
					})),
					Some(text),
					"execution.failure_pending",
				)
			}
		};
		let m = &delivery.metadata;
		let mut tx = self.pool.begin(scope).await?;
		let query = Query::update()
			.table(a("task_executions"))
			.value(a("phase"), state.phase().as_str())
			.value_expr(a("pending"), Expr::value(crate::domain::run_state::encode(&state, &RecoveryState::default())?))
			.value_expr(a("revision"), Expr::col(a("revision")).add(Expr::value(1)))
			.value_expr(a("updated_at"), Expr::current_timestamp())
			.value_expr(a("lease_owner"), Expr::cust("NULL"))
			.value_expr(a("lease_until"), Expr::cust("NULL"))
			.and_where(Expr::col(a("id")).eq(Expr::value(m.id)))
			.and_where(Expr::col(a("revision")).eq(Expr::value(m.revision)))
			.and_where(Expr::col(a("lease_owner")).eq(Expr::value(worker)))
			.and_where(Expr::cust(
				"lease_until>CURRENT_TIMESTAMP AND phase='WAITING' AND set_config('aidash.input_ledger_worker','true',true)='true'",
			))
			.to_string(PostgresQueryBuilder);
		let updated = crate::database::native::query(&query)
			.execute(&mut *tx)
			.await?;
		if updated.rows_affected() != 1 {
			return Err(Error::Conflict(
				"failure delivery lease or revision lost".into(),
			));
		}
		if state.phase().is_terminal() {
			// Before the first event; see `lock_end`.
			crate::apps::execution::repositories::subnodes::lock_for_end(&mut tx, m.id).await?;
			crate::apps::execution::models::HumanRequest::close_for_run(tx.as_mut(), m.id).await?;
		}
		self.run_event(
			&mut tx,
			m.id,
			m.is_local(self.deployment.as_str()).then_some(m.thread_id),
			kind,
			json!({"run_id":m.id,"task_id":m.task_id,"phase":state.phase(),"delivery_error":error}),
		)
		.await?;
		// Failure delivery can itself be the terminal transition.
		// Only a cancellation ends what the creator built (#1012).
		if state.phase() == aidash_domain::RunPhase::Cancelled {
			self.subnode_control_in(
				&mut tx,
				m.id,
				aidash_domain::RunControlAction::Cancel,
				aidash_domain::ControlSource::System,
				Some("creator_cancelled"),
			)
			.await?;
		}
		// The failed worker's lease expired; its attempt cannot be finished by it.
		let closed = if state.phase().is_terminal() {
			crate::apps::execution::repositories::inference::close_pending(
				tx.as_mut(),
				self.deployment.as_str(),
				m.id,
				None,
			)
			.await?
		} else {
			Vec::new()
		};
		tx.commit().await?;
		crate::apps::execution::repositories::inference::record_interruptions(&closed);
		Ok(())
	}
	pub async fn inspect_run(&self, scope: &TenancyScope, id: Uuid) -> Result<RunInspection> {
		let raw: RawRun = aidash_server::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(a("task_executions"))
				.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(self.pool.in_scope(scope))
		.await?
		.ok_or_else(|| Error::NotFound("run".into()))?;
		Ok(raw.inspect())
	}
	pub async fn inspect_runs(&self, scope: &TenancyScope) -> Result<Vec<RunInspection>> {
		let rows: Vec<RawRun> = aidash_server::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(a("task_executions"))
				.order_by(a("updated_at"), reinhardt::query::Order::Desc)
				.limit(500)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(self.pool.in_scope(scope))
		.await?;
		Ok(rows.into_iter().map(RawRun::inspect).collect())
	}
}
