use super::*;
use chrono::{DateTime, Utc};
use reinhardt::query::{Alias, Condition, Expr, JoinType, PostgresQueryBuilder, Query};
fn a(name: &str) -> Alias {
	Alias::new(name)
}

pub(crate) type RecoveryCursor = Option<(DateTime<Utc>, Uuid)>;
#[derive(Default)]
pub(crate) struct RecoveryCursors {
	pub execution: tokio::sync::Mutex<RecoveryCursor>,
	pub delivery: tokio::sync::Mutex<RecoveryCursor>,
}

pub(crate) struct FailureDelivery {
	pub metadata: RunMetadata,
	pub target: FailureTarget,
}

/// The same ordering and Area blockers govern claims and deferred notifications.
pub(crate) fn run_unblocked() -> Condition {
	let earlier = Query::select()
		.expr(Expr::val(1))
		.from_as(Alias::new("core_runs"), Alias::new("mine"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("core_runs"), Alias::new("earlier")),
			Condition::all()
				.add(
					Expr::col((Alias::new("mine"), Alias::new("area_id")))
						.equals((Alias::new("earlier"), Alias::new("area_id"))),
				)
				.add(
					Expr::col((Alias::new("mine"), Alias::new("generation")))
						.equals((Alias::new("earlier"), Alias::new("generation"))),
				)
				.add(
					reinhardt::query::SimpleExpr::from(Expr::col((
						Alias::new("earlier"),
						Alias::new("sequence"),
					)))
					.lt(Expr::col((Alias::new("mine"), Alias::new("sequence")))),
				),
		)
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("runs"), Alias::new("previous")),
			Expr::col((Alias::new("earlier"), Alias::new("run_id")))
				.equals((Alias::new("previous"), Alias::new("id"))),
		)
		.and_where(
			Expr::col((Alias::new("mine"), Alias::new("run_id")))
				.equals((Alias::new("runs"), Alias::new("id"))),
		)
		.and_where(
			Expr::col((Alias::new("previous"), Alias::new("phase"))).is_not_in([
				"COMPLETED",
				"FAILED",
				"CANCELLED",
			]),
		)
		.to_owned();
	let unready = Query::select()
		.expr(Expr::val(1))
		.from_as(Alias::new("core_runs"), Alias::new("q"))
		.join(
			JoinType::InnerJoin,
			reinhardt::query::TableRef::table_alias(Alias::new("core_areas"), Alias::new("a")),
			Expr::col((Alias::new("q"), Alias::new("area_id")))
				.equals((Alias::new("a"), Alias::new("id"))),
		)
		.and_where(
			Expr::col((Alias::new("q"), Alias::new("run_id")))
				.equals((Alias::new("runs"), Alias::new("id"))),
		)
		.and_where(
			Expr::col((Alias::new("q"), Alias::new("initialized")))
				.eq(reinhardt::query::Expr::value(false)),
		)
		.and_where(
			Expr::col((Alias::new("a"), Alias::new("state")))
				.ne(reinhardt::query::Expr::value("active")),
		)
		.to_owned();
	Condition::any()
		.add(Expr::col(a("control")).eq(reinhardt::query::Expr::value("CANCELLED")))
		.add(
			Condition::all()
				.add(Expr::exists(earlier).not())
				.add(Expr::exists(unready).not()),
		)
}

impl Store {
	pub(crate) async fn claim_failure_delivery(
		&self,
		worker: Uuid,
		seconds: i32,
	) -> Result<Option<FailureDelivery>> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
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
			.from(a("runs"))
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
			let target = if raw.control == RunControl::Cancelled {
				FailureTarget::Cancelled
			} else {
				target
			};
			let metadata: Option<RunMetadata> = aidash_server::database::query_as(
				&Query::update()
					.table(a("runs"))
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
		delivery: &FailureDelivery,
		worker: Uuid,
		result: Result<TaskStatus>,
	) -> Result<()> {
		let (state, error, kind) = match result {
			Ok(status) => {
				let (state, kind) = match status {
					TaskStatus::Completed => {
						(RunState::Completed(TerminalState {}), "run.completed")
					}
					TaskStatus::Failed => (RunState::Failed(TerminalState {}), "run.failed"),
					TaskStatus::Cancelled | TaskStatus::Abandoned => {
						(RunState::Cancelled(TerminalState {}), "run.cancelled")
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
				(state, None, kind)
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
					"run.failure_pending",
				)
			}
		};
		let m = &delivery.metadata;
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let query = Query::update()
			.table(a("runs"))
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
		self.event(
			&mut tx,
			(m.home_node == self.node_id).then_some(m.workspace_id),
			kind,
			json!({"run_id":m.id,"task_id":m.task_id,"phase":state.phase(),"delivery_error":error}),
		)
		.await?;
		tx.commit().await?;
		Ok(())
	}
	pub async fn inspect_run(&self, id: Uuid) -> Result<RunInspection> {
		let raw: RawRun = aidash_server::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(a("runs"))
				.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&self.pool)
		.await?
		.ok_or_else(|| Error::NotFound("run".into()))?;
		Ok(raw.inspect())
	}
	pub async fn inspect_runs(&self) -> Result<Vec<RunInspection>> {
		let rows: Vec<RawRun> = aidash_server::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(a("runs"))
				.order_by(a("updated_at"), reinhardt::query::Order::Desc)
				.limit(500)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?;
		Ok(rows.into_iter().map(RawRun::inspect).collect())
	}
}
