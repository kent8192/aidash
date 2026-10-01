use super::*;
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{Alias, Condition, Expr, JoinType, PostgresQueryBuilder, Query};
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
		.join_as(
			JoinType::InnerJoin,
			Alias::new("core_runs"),
			Alias::new("earlier"),
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
					Expr::col((Alias::new("earlier"), Alias::new("sequence")))
						.lt(Expr::col((Alias::new("mine"), Alias::new("sequence")))),
				),
		)
		.join_as(
			JoinType::InnerJoin,
			Alias::new("runs"),
			Alias::new("previous"),
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
		.join_as(
			JoinType::InnerJoin,
			Alias::new("core_areas"),
			Alias::new("a"),
			Expr::col((Alias::new("q"), Alias::new("area_id")))
				.equals((Alias::new("a"), Alias::new("id"))),
		)
		.and_where(
			Expr::col((Alias::new("q"), Alias::new("run_id")))
				.equals((Alias::new("runs"), Alias::new("id"))),
		)
		.and_where(Expr::col((Alias::new("q"), Alias::new("initialized"))).eq(false))
		.and_where(Expr::col((Alias::new("a"), Alias::new("state"))).ne("active"))
		.to_owned();
	Condition::any()
		.add(Expr::col(a("control")).eq("CANCELLED"))
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
		let mut tx = self.pool.begin().await?;
		let now: DateTime<Utc> = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("CURRENT_TIMESTAMP"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		let mut cursor = self.recovery_cursors.delivery.lock().await;
		let mut query = Query::select()
			.column(sea_orm::sea_query::Asterisk)
			.from(a("runs"))
			.and_where(Expr::col(a("phase")).eq("WAITING"))
			.and_where(Expr::col(a("control")).ne("PAUSED"))
			.and_where(Expr::col(a("revision")).lt(i64::MAX - 2))
			.and_where(Expr::cust(
				"pending -> 'data' ->> 'reason' = 'failure_delivery' AND (lease_until IS NULL OR lease_until <= CURRENT_TIMESTAMP)",
			))
			.order_by(a("updated_at"), sea_orm::sea_query::Order::Asc)
			.order_by(a("id"), sea_orm::sea_query::Order::Asc)
			.limit(128)
			.lock_with_behavior(
				sea_orm::sea_query::LockType::Update,
				sea_orm::sea_query::LockBehavior::SkipLocked,
			)
			.to_owned();
		if let Some((at, id)) = *cursor {
			query.cond_where(
				sea_orm::sea_query::Condition::any()
					.add(Expr::col(a("updated_at")).gt(at))
					.add(
						sea_orm::sea_query::Condition::all()
							.add(Expr::col(a("updated_at")).eq(at))
							.add(Expr::col(a("id")).gt(id)),
					),
			);
		}
		let rows: Vec<RawRun> = sqlx::query_as(&query.to_string(PostgresQueryBuilder))
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
			let metadata: Option<RunMetadata> = sqlx::query_as(
				&Query::update()
					.table(a("runs"))
					.value(a("lease_owner"), worker)
					.value(
						a("lease_until"),
						Expr::cust_with_values(
							"CURRENT_TIMESTAMP + make_interval(secs => $1)",
							[seconds],
						),
					)
					.value(a("revision"), Expr::col(a("revision")).add(1))
					.value(a("ledger_worker_ready"), true)
					.and_where(Expr::col(a("id")).eq(raw.id))
					.and_where(Expr::col(a("revision")).eq(raw.revision))
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
		let mut tx = self.pool.begin().await?;
		let query = Query::update()
			.table(a("runs"))
			.value(a("phase"), state.phase().as_str())
			.value(a("pending"), Expr::cust("$4"))
			.value(a("revision"), Expr::col(a("revision")).add(1))
			.value(a("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
			.value(a("lease_owner"), Expr::cust("NULL"))
			.value(a("lease_until"), Expr::cust("NULL"))
			.and_where(Expr::cust(
				"id=$1 AND revision=$2 AND lease_owner=$3 AND lease_until>CURRENT_TIMESTAMP AND phase='WAITING' AND set_config('aidash.input_ledger_worker','true',true)='true'",
			))
			.to_string(PostgresQueryBuilder);
		let updated = sqlx::query(&query)
			.bind(m.id)
			.bind(m.revision)
			.bind(worker)
			.bind(crate::domain::run_state::encode(
				&state,
				&RecoveryState::default(),
			)?)
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
		let raw: RawRun = sqlx::query_as(
			&Query::select()
				.column(sea_orm::sea_query::Asterisk)
				.from(a("runs"))
				.and_where(Expr::col(a("id")).eq(id))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&self.pool)
		.await?
		.ok_or_else(|| Error::NotFound("run".into()))?;
		Ok(raw.inspect())
	}
	pub async fn inspect_runs(&self) -> Result<Vec<RunInspection>> {
		let rows: Vec<RawRun> = sqlx::query_as(
			&Query::select()
				.column(sea_orm::sea_query::Asterisk)
				.from(a("runs"))
				.order_by(a("updated_at"), sea_orm::sea_query::Order::Desc)
				.limit(500)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?;
		Ok(rows.into_iter().map(RawRun::inspect).collect())
	}
	/// Typed scheduling shared by notification and recovery. All deadlines are
	/// parsed before comparison; malformed JSON never reaches a SQL cast.
	pub(crate) async fn state_due_in(
		tx: &mut Transaction<'_, Postgres>,
		raw: &RawRun,
		now: DateTime<Utc>,
		node_id: &str,
	) -> Result<Option<DateTime<Utc>>> {
		let m = &raw.metadata;
		if m.phase.is_terminal() || m.control == RunControl::Paused {
			return Ok(None);
		}
		if let Some(until) = m.lease_until.filter(|d| *d > now) {
			return Ok(Some(until));
		}
		if m.control == RunControl::Cancelled {
			return Ok(Some(now));
		}
		let unblocked: bool = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::exists(
					Query::select()
						.expr(Expr::val(1))
						.from(a("runs"))
						.and_where(Expr::col(a("id")).eq(m.id))
						.cond_where(run_unblocked())
						.to_owned(),
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **tx)
		.await?;
		if !unblocked {
			return Ok(None);
		}
		let Ok((state, recovery)) = crate::domain::run_state::decode(m.phase, raw.pending.clone())
		else {
			return Ok(Some(now));
		};
		let retry_at = recovery.retry.map(|r| r.at).filter(|d| *d > now);
		let due = match &state {
			RunState::Waiting(wait) => {
				if let Some(id) = wait.request_id() {
					let answered: bool = sqlx::query_scalar(
						&Query::select()
							.expr(Expr::exists(
								Query::select()
									.expr(Expr::val(1))
									.from(a("human_requests"))
									.and_where(Expr::col(a("id")).eq(id))
									.and_where(Expr::col(a("response")).is_not_null())
									.to_owned(),
							))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut **tx)
					.await?;
					if answered { Some(now) } else { wait.deadline() }
				} else if let WaitingState::CoreApproval { approval_id, .. } = wait.as_ref() {
					let approval: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
						&Query::select()
							.columns([a("state"), a("expires_at")])
							.from(a("core_records"))
							.and_where(Expr::col(a("id")).eq(*approval_id))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_optional(&mut **tx)
					.await?;
					match approval {
						Some((status, expires)) if status == "pending" => expires,
						Some(_) => Some(now),
						None => Some(now),
					}
				} else if m.home_node == node_id
					&& matches!(wait.as_ref(), WaitingState::Dependencies { .. })
				{
					let mut dependencies = Query::select()
						.expr(Expr::val(1))
						.from(a("task_dependencies"))
						.inner_join(
							a("tasks"),
							Expr::col((a("tasks"), a("id")))
								.equals((a("task_dependencies"), a("dependency_id"))),
						)
						.and_where(Expr::col((a("task_dependencies"), a("task_id"))).eq(m.task_id))
						.to_owned();
					let mut incomplete = dependencies.clone();
					incomplete.and_where(
						Expr::col((a("tasks"), a("status"))).ne(TaskStatus::Completed.as_str()),
					);
					dependencies.and_where(Expr::col((a("tasks"), a("status"))).is_in([
						TaskStatus::Failed.as_str(),
						TaskStatus::Cancelled.as_str(),
						TaskStatus::Abandoned.as_str(),
					]));
					let ready: bool = sqlx::query_scalar(
						&Query::select()
							.expr(
								Expr::exists(incomplete)
									.not()
									.or(Expr::exists(dependencies)),
							)
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut **tx)
					.await?;
					if ready { Some(now) } else { wait.deadline() }
				} else {
					wait.deadline()
				}
			}
			_ => Some(now),
		};
		// Invalid Context also requires a disposition even when an ordinary wait has
		// no deadline. Pause/ownership checks above still take priority.
		let due = if raw.decode().is_err() && !state.failure_delivery() {
			Some(now)
		} else {
			due
		};
		Ok(due.map(|d| retry_at.map_or(d, |r| d.max(r))))
	}
	pub(crate) async fn fail_invalid_in(
		tx: &mut Transaction<'_, Postgres>,
		raw: &RawRun,
		worker: Uuid,
		seconds: i32,
		reason: &str,
		node_id: &str,
	) -> Result<()> {
		let m = &raw.metadata;
		let target = if m.control == RunControl::Cancelled {
			FailureTarget::Cancelled
		} else {
			FailureTarget::Failed
		};
		let now: DateTime<Utc> = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("CURRENT_TIMESTAMP"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **tx)
		.await?;
		let state = RunState::Waiting(Box::new(WaitingState::FailureDelivery {
			target,
			wake_at: now,
			last_delivery_error: None,
		}));
		// Claim repair ownership first. Row locks alone do not authorize an update.
		let owned: Option<RunMetadata> = sqlx::query_as(
			&Query::update()
				.table(a("runs"))
				.value(a("lease_owner"), worker)
				.value(a("lease_until"), Expr::cust_with_values(
					"CURRENT_TIMESTAMP + make_interval(secs => $1)", [seconds],
				))
				.value(a("revision"), Expr::col(a("revision")).add(1))
				.value(a("ledger_worker_ready"), true)
				.and_where(Expr::col(a("id")).eq(m.id))
				.and_where(Expr::col(a("revision")).eq(m.revision))
				.and_where(Expr::col(a("phase")).is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
				.and_where(Expr::col(a("control")).ne("PAUSED"))
				.and_where(Expr::cust(
					"(lease_until IS NULL OR lease_until <= CURRENT_TIMESTAMP) AND set_config('aidash.input_ledger_worker','true',true)='true'",
				))
				.returning_all()
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **tx)
		.await?;
		let Some(owned) = owned else {
			return Ok(());
		};
		let query = Query::update()
			.table(a("runs"))
			.value(a("phase"), "WAITING")
			.value(a("pending"), Expr::cust("$4"))
			.value(a("error"), reason)
			.value(a("revision"), Expr::col(a("revision")).add(1))
			.value(a("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
			.value(a("lease_owner"), Expr::cust("NULL"))
			.value(a("lease_until"), Expr::cust("NULL"))
			.and_where(Expr::cust(
				"id=$1 AND revision=$2 AND lease_owner=$3 AND lease_until>CURRENT_TIMESTAMP",
			))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&query)
			.bind(m.id)
			.bind(owned.revision)
			.bind(worker)
			.bind(crate::domain::run_state::encode(
				&state,
				&RecoveryState::default(),
			)?)
			.execute(&mut **tx)
			.await?;
		// The Run trigger writes activation intent in this same transaction. The
		// failure event needs no potentially damaged Context or effect payload.
		let event = Query::insert()
			.into_table(a("events"))
			.columns([
				a("id"),
				a("node_id"),
				a("workspace_id"),
				a("kind"),
				a("data"),
			])
			.values_panic([
				Uuid::new_v4().into(),
				Expr::cust("$1"),
				Expr::cust("$2"),
				"run.invalid_state".into(),
				Expr::cust("$3"),
			])
			.to_string(PostgresQueryBuilder);
		let node = node_id;
		sqlx::query(&event)
			.bind(node)
			.bind((node == m.home_node).then_some(m.workspace_id))
			.bind(json!({"run_id":m.id,"task_id":m.task_id,"category":reason}))
			.execute(&mut **tx)
			.await?;
		Ok(())
	}
}
