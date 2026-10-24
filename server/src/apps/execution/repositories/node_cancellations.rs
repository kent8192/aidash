//! Durable Node stop orchestration. The deletion transaction snapshots identities;
//! recovery invokes the existing lifecycle owners without live Node admission.
use super::super::models::{Run as RunRecord, node_cancellation_jobs::NodeCancellationJob};
use crate::{Error, Result, federation::Federation};
use aidash_application::{
	ports::activation::SchedulingScope,
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::{RunControl, RunControlAction, TaskStatus, run_state::RawRun};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use uuid::Uuid;

pub(crate) mod home_commands;
mod snapshot;

const BATCH: usize = 32;

/// Called immediately after NodeDeletion insertion, under its policy/capacity
/// locks. The snapshot survives restore; subsequent work is never selected.
pub(crate) async fn record(
	f: &Federation,
	tx: &mut dyn TransactionExecutor,
	deletion: &crate::apps::identity::models::NodeDeletion,
) -> Result<()> {
	let address = format!("{}/{}", f.config.deployment, deletion.node);
	for (kind, table, predicate, pending) in snapshot::selectors(
		&deletion.node,
		&address,
		f.config.deployment.as_str(),
		deletion.id,
	) {
		let select = Query::select()
			.expr(Expr::value(format!("{}:{kind}:", deletion.id)).binary(
				reinhardt::query::BinOper::PgOperator(
					reinhardt::query::types::PgBinOper::Concatenate,
				),
				Expr::col("id").cast_as("text"),
			))
			.expr(Expr::value(deletion.id))
			.expr(Expr::value(deletion.tenant.clone()))
			.expr(Expr::value(deletion.node.clone()))
			.expr(Expr::value(kind))
			.column(Alias::new("id"))
			.expr(if kind == "python" {
				SimpleExpr::FunctionCall(
					Alias::new("to_jsonb").into_iden(),
					vec![Expr::col("core_records").into()],
				)
			} else {
				Expr::value(serde_json::Value::Null).into()
			})
			.expr(Expr::current_timestamp())
			.from(Alias::new(table))
			.and_where(Expr::col("tenant").eq(Expr::value(deletion.tenant.clone())))
			.and_where(predicate)
			.and_where(pending)
			.to_owned();
		let (sql, values) = Query::insert()
			.into_table(Alias::new("node_cancellation_jobs"))
			.columns(
				[
					"id",
					"deletion_id",
					"tenant",
					"node",
					"kind",
					"work_id",
					"binding",
					"retry_at",
				]
				.map(Alias::new),
			)
			.from_subquery(select)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	// A decided participant owns the original Run revision. Keep its recovery
	// intact; snapshot-based admission fences remain effective after restore.
	if crate::transactions::gate::try_read_in(tx).await?.is_none() {
		return Ok(());
	}

	// Run control is irrevocable and the old worker token cannot survive restore.
	let runs: Vec<RawRun> = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("tenant").eq(Expr::value(deletion.tenant.clone())))
			.and_where(
				Expr::col("id").in_subquery(
					Query::select()
						.column(Alias::new("work_id"))
						.from(Alias::new("node_cancellation_jobs"))
						.and_where(Expr::col("deletion_id").eq(Expr::value(deletion.id)))
						.and_where(Expr::col("kind").eq("run"))
						.to_owned(),
				),
			)
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	for raw in runs {
		if !raw.metadata.phase.is_terminal() && raw.metadata.control != RunControl::Cancelled {
			f.store
				.control_in_executor(tx, raw.metadata.id, RunControlAction::Cancel)
				.await?;
		}
		if let Some(token) = raw.metadata.lease_owner {
			RunRecord::worker_context(tx).await?;
			RunRecord::release_worker(tx, raw.metadata.id, token).await?;
		}
	}
	for kind in ["grant", "intent", "admission"] {
		let ids: Vec<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("work_id"))
				.from(Alias::new("node_cancellation_jobs"))
				.and_where(Expr::col("deletion_id").eq(Expr::value(deletion.id)))
				.and_where(Expr::col("kind").eq(Expr::value(kind)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut *tx)
		.await?;
		for id in ids {
			if kind == "grant" {
				crate::apps::identity::repositories::remote_grants::persistence::revoke_for_node_in(
					&f.store,
					tx,
					&deletion.tenant,
					id,
				)
				.await?;
			} else if kind == "admission" {
				crate::apps::identity::repositories::peer_admission::cancel_in(
					tx,
					&deletion.tenant,
					id,
				)
				.await?;
			} else {
				crate::apps::execution::generation::repositories::foreign::home::set_cancelled_in(
					tx, id,
				)
				.await?;
			}
		}
	}
	Ok(())
}

/// Assignment admission retains this fence even after Node restoration. Reads
/// and stop/control operations deliberately do not call this method.
pub(crate) async fn require_task_open(tx: &mut dyn TransactionExecutor, task: Uuid) -> Result<()> {
	require_work_open(tx, "task", task).await
}
pub(crate) async fn require_grant_open(
	tx: &mut dyn TransactionExecutor,
	grant: Uuid,
) -> Result<()> {
	require_work_open(tx, "grant", grant).await
}
pub(crate) async fn require_admission_open(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
) -> Result<()> {
	require_work_open(tx, "admission", id).await
}
pub(crate) async fn require_run_open(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<()> {
	require_work_open(tx, "run", run).await
}
pub(crate) fn run_open_condition(run: SimpleExpr, tenant: SimpleExpr) -> SimpleExpr {
	Expr::exists(
		Query::select()
			.expr(Expr::value(1_i32))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(Expr::col("kind").eq("run"))
			.and_where(Expr::col("work_id").eq(run))
			.and_where(Expr::col("tenant").eq(tenant))
			.to_owned(),
	)
	.not()
}
/// Fresh atomic mutations use current lifecycle authority. Exact decided
/// finish/apply never calls this guard and retains its original decision proof.
pub(crate) async fn require_fresh_run(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	run: Uuid,
) -> Result<()> {
	require_run_open(tx, run).await?;
	let raw: RawRun = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	if raw.metadata.control != RunControl::Active || raw.metadata.phase.is_terminal() {
		return Err(Error::Forbidden);
	}
	require_admission_open(tx, run).await?;
	let cancelled: Option<bool> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("cancelled"))
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(tx)
	.await?;
	if cancelled == Some(true) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub(crate) async fn require_fresh_task(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	task: Uuid,
) -> Result<()> {
	require_task_open(tx, task).await?;
	let task: aidash_domain::Task = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("tasks"))
			.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
			.and_where(Expr::col("id").eq(Expr::value(task)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(tx)
	.await?;
	if matches!(
		task.status,
		TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Abandoned
	) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
pub(crate) async fn require_generation_open(
	tx: &mut dyn TransactionExecutor,
	request: Uuid,
) -> Result<()> {
	require_work_open(tx, "generation", request).await
}
async fn require_work_open(tx: &mut dyn TransactionExecutor, kind: &str, id: Uuid) -> Result<()> {
	let pending: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(Expr::col("kind").eq(Expr::value(kind.to_owned())))
			.and_where(Expr::col("work_id").eq(Expr::value(id)))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(tx)
	.await?;
	if pending.is_some() {
		return Err(Error::Conflict("Node cancellation is pending".into()));
	}
	Ok(())
}

/// Each tick visits at most 32 due jobs. Reserving retry_at before effects is a
/// persistent cursor: a broken first job cannot starve the next page, and restart
/// needs no in-memory progress. Owners serialize concurrent retry after expiry.
pub(crate) async fn reconcile(f: &Federation) -> Result<()> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::GenerationReconcile);
	let jobs = f
		.store
		.pool
		.atomic(&scope, async |tx| {
			// Selection and reservation use the database clock, so host clock
			// drift cannot postpone a due stop indefinitely after restart.
			let now: chrono::DateTime<chrono::Utc> = crate::database::native::query_scalar(
				&Query::select()
					.expr(Expr::current_timestamp())
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			Ok(NodeCancellationJob::objects()
				.filter(NodeCancellationJob::field_retry_at().lte(now))
				.order_by(&["retry_at", "id"])
				.limit(BATCH)
				.all_with_executor(tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?)
		})
		.await?;
	for job in jobs {
		let scope = TenancyScope::tenant(&job.tenant)?;
		let reserved = crate::database::native::query(
			&Query::update()
				.table(Alias::new("node_cancellation_jobs"))
				.value_expr(
					Alias::new("retry_at"),
					SimpleExpr::FunctionCall(Alias::new("clock_timestamp").into_iden(), vec![])
						.add(Expr::value("30 seconds").cast_as("interval")),
				)
				.and_where(Expr::col("id").eq(Expr::value(job.id.clone())))
				.and_where(Expr::col("retry_at").lte(SimpleExpr::FunctionCall(
					Alias::new("clock_timestamp").into_iden(),
					vec![],
				)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.in_scope(&scope))
		.await?
		.rows_affected();
		if reserved == 0 {
			continue;
		}
		match process(f, &job).await {
			Ok(true) => {
				crate::database::native::query(
					&Query::delete()
						.from_table(Alias::new("node_cancellation_jobs"))
						.and_where(Expr::col("id").eq(Expr::value(job.id)))
						.to_string(PostgresQueryBuilder),
				)
				.execute(f.store.pool.in_scope(&scope))
				.await?;
			}
			Ok(false) => {}
			Err(error) => tracing::warn!(job=%job.id,%error,"Node cancellation remains queued"),
		}
	}
	f.notify.notify_waiters();
	Ok(())
}
async fn process(f: &Federation, job: &NodeCancellationJob) -> Result<bool> {
	match job.kind.as_str() {
		"admission" => {
			let mut tx = f
				.store
				.pool
				.begin(&TenancyScope::tenant(&job.tenant)?)
				.await?;
			crate::authorization::Authorization::load_with_mode(&mut tx, &job.tenant, true).await?;
			crate::transactions::gate::read_in(&mut tx).await?;
			crate::apps::identity::repositories::peer_admission::cancel_in(
				tx.as_mut(),
				&job.tenant,
				job.work_id,
			)
			.await?;
			tx.commit().await?;
			Ok(true)
		}
		"run" => stop_run(f, job).await,
		"generation" => stop_generation(f, job).await,
		"grant" => {
			crate::apps::identity::repositories::home_execution::cancel_deleted_node_grant(
				f,
				&job.tenant,
				&job.node,
				job.work_id,
			)
			.await
		}
		"intent" => {
			// The existing foreign maintenance owner durably reserves retries and
			// authenticates the original exact intent before sending its cancel.
			let scope = TenancyScope::tenant(&job.tenant)?;
			let mut tx = f.store.pool.begin(&scope).await?;
			crate::authorization::Authorization::load_with_mode(&mut tx, &job.tenant, true).await?;
			crate::transactions::gate::read_in(&mut tx).await?;
			crate::apps::execution::generation::repositories::foreign::home::set_cancelled_in(
				tx.as_mut(),
				job.work_id,
			)
			.await?;
			tx.commit().await?;
			let row: Option<(bool, serde_json::Value)> = crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("cancel_delivered"), Alias::new("binding")])
					.from(Alias::new("generation_remote_intents"))
					.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["cancel_delivered", "binding"])
			.fetch_optional(f.store.pool.in_scope(&scope))
			.await?;
			let Some((delivered, binding)) = row else {
				return Err(Error::Conflict(
					"cancellation intent owner disappeared".into(),
				));
			};
			if delivered {
				return Ok(true);
			}
			let intent: aidash_domain::generation::intent::Intent =
				serde_json::from_value(binding)?;
			aidash_application::generation::foreign::maintenance::deliver_cancel(
				&crate::bootstrap::generation_foreign_maintenance(f),
				job.work_id,
				&intent.target_node,
			)
			.await?;
			let delivered: bool = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("cancel_delivered"))
					.from(Alias::new("generation_remote_intents"))
					.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(f.store.pool.in_scope(&scope))
			.await?;
			Ok(delivered)
		}
		"operation" => stop_operation(f, job).await,
		"python" => stop_python(f, job).await,
		"task" => stop_task(f, job).await,
		_ => Err(Error::Invalid("unknown Node cancellation job".into())),
	}
}
async fn stop_generation(f: &Federation, job: &NodeCancellationJob) -> Result<bool> {
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::tenant(&job.tenant)?)
		.await?;
	crate::authorization::Authorization::load_with_mode(&mut tx, &job.tenant, true).await?;
	crate::transactions::gate::read_in(&mut tx).await?;
	let request = crate::generation::lifecycle::load(&mut tx, &job.tenant, job.work_id).await?;
	if request.node.as_str() != job.node {
		return Err(Error::Forbidden);
	}
	if !request.quota_released
		|| matches!(
			request.status.as_str(),
			"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
		) {
		let status = if matches!(
			request.status.as_str(),
			"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
		) {
			"STOPPED"
		} else {
			&request.status
		};
		crate::generation::lifecycle::transition(
			f,
			&mut tx,
			&request,
			status,
			"node-lifecycle",
			"owning Node deleted",
		)
		.await?;
	}
	tx.commit().await?;
	Ok(true)
}
async fn stop_run(f: &Federation, job: &NodeCancellationJob) -> Result<bool> {
	let scope = TenancyScope::tenant(&job.tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	crate::transactions::gate::read_in(&mut tx).await?;
	let raw: Option<RawRun> = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
			.and_where(Expr::col("tenant").eq(Expr::value(job.tenant.clone())))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(mut raw) = raw else {
		return Err(Error::Conflict("cancellation Run owner disappeared".into()));
	};
	if raw.metadata.phase.is_terminal() {
		if let Some(token) = raw.metadata.lease_owner {
			RunRecord::worker_context(tx.as_mut()).await?;
			RunRecord::release_worker(tx.as_mut(), raw.metadata.id, token).await?;
		}
		tx.commit().await?;
		return Ok(true);
	}
	if raw.metadata.control != RunControl::Cancelled {
		f.store
			.control_in(&mut tx, raw.metadata.id, RunControlAction::Cancel)
			.await?;
		if let Some(token) = raw.metadata.lease_owner {
			RunRecord::worker_context(tx.as_mut()).await?;
			RunRecord::release_worker(tx.as_mut(), raw.metadata.id, token).await?;
		}
		raw = crate::database::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
	}
	let token = Uuid::new_v4();
	let owned = crate::bootstrap::activation_scheduling_scope(tx.as_mut())
		.repair_lease(&raw.metadata, token, 30)
		.await?;
	tx.commit().await?;
	let Some(owned) = owned else { return Ok(false) };
	raw.metadata = owned.clone();
	if aidash_domain::federation::execution::settlement::has_saved_continuation(&raw) {
		let result = settle_saved_run(f, &scope, raw, token).await;
		if result.is_err() {
			f.store.release_lease(&scope, job.work_id, token).await?;
		}
		return result;
	}
	f.store
		.cancel_owned_execution(&scope, &owned, token)
		.await?;
	Ok(true)
}

/// Resume only the saved settlement, after releasing the cancellation locks.
/// Home ledgers make a lost reply retryable without repeating inference or effects.
async fn settle_saved_run(
	f: &Federation,
	scope: &TenancyScope,
	raw: RawRun,
	token: Uuid,
) -> Result<bool> {
	use aidash_domain::{RunState, WaitingState, run_state::TerminalState};
	let (state, _) = aidash_domain::run_state::decode(raw.metadata.phase, raw.pending.clone())?;
	if let RunState::Waiting(wait) = &state
		&& let WaitingState::FailureDelivery { target, .. } = **wait
	{
		let result = async {
			f.deliver_run_message_metadata(&raw.metadata).await?;
			f.transition_terminal_metadata(&raw.metadata, target.task_status(), None)
				.await
				.map(|task| task.status)
		}
		.await;
		let settled = result.is_ok();
		f.store
			.finish_failure_delivery(
				scope,
				&super::store::run_state::FailureDelivery {
					metadata: raw.metadata,
					target,
				},
				token,
				result,
			)
			.await?;
		return Ok(settled);
	}
	let mut run = raw.decode()?;
	let home = crate::federation::Home::new(f.clone(), run.clone());
	let task = home.task().await?;
	let artifact = aidash_domain::ArtifactInput {
		kind: "text".into(),
		name: aidash_application::execution::result_artifact_name(&task.title),
		content: serde_json::json!(run.state.tool()?.response.text),
	};
	let task = home
		.complete(&format!("{}:complete", run.id), &artifact)
		.await?;
	if task.status != TaskStatus::Completed {
		return Err(Error::Conflict("saved completion is not settled".into()));
	}
	run.state = RunState::Completed(TerminalState {});
	f.store
		.save_run(scope, &run, token, "run.completed")
		.await?;
	Ok(true)
}
async fn stop_task(f: &Federation, job: &NodeCancellationJob) -> Result<bool> {
	let mut tx = f
		.store
		.pool
		.begin(&TenancyScope::tenant(&job.tenant)?)
		.await?;
	crate::transactions::gate::read_in(&mut tx).await?;
	// Delegated Home ledger closure belongs to the grant owner, after its stop RPC.
	let pending: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(Expr::col("deletion_id").eq(Expr::value(job.deletion_id)))
			.and_where(Expr::col("kind").eq(Expr::value("grant")))
			.and_where(
				Expr::col("work_id").in_subquery(
					Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("authorization_remote_grants"))
						.and_where(Expr::col("tenant").eq(Expr::value(job.tenant.clone())))
						.and_where(Expr::col("task_id").eq(Expr::value(job.work_id)))
						.to_owned(),
				),
			)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	if pending.is_some() {
		return Ok(false);
	}
	let task: aidash_domain::Task = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("tasks"))
			.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	// A local saved finalization/failure still owns this exact Home Task. Its
	// Run job may be retrying transport, so this tick must leave the Task intact.
	let workspace: aidash_domain::Workspace = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("id").eq(Expr::value(task.workspace_id)))
			.and_where(Expr::col("tenant").eq(Expr::value(job.tenant.clone())))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	let saved: Vec<RawRun> = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("tenant").eq(Expr::value(job.tenant.clone())))
			.and_where(Expr::col("task_id").eq(Expr::value(task.id)))
			.and_where(Expr::col("workspace_id").eq(Expr::value(task.workspace_id)))
			.and_where(Expr::col("home_node").eq(Expr::value(format!(
				"{}/{}",
				f.config.deployment, workspace.node
			))))
			.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	if saved
		.iter()
		.any(aidash_domain::federation::execution::settlement::has_saved_continuation)
	{
		return Ok(false);
	}
	if !matches!(
		task.status,
		TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Abandoned
	) {
		f.store
			.transition_in(
				&mut tx,
				task.id,
				task.revision,
				task.owner.as_deref().unwrap_or("node-lifecycle"),
				TaskStatus::Cancelled,
			)
			.await?;
	}
	tx.commit().await?;
	Ok(true)
}

use aidash_application::federation::inbound::RequestDigest;
use aidash_domain::{federation::route::Sender, nodes::NodeAddress};

/// Only the exact two stop operations can use retained Node routing. Parsing
/// their real strict request types also rejects extra effect-bearing fields.
enum Stop {
	Generation(Uuid),
	Execution { admission: Uuid, grant: Uuid },
}
fn stop(request: &RequestDigest<'_>) -> Option<Stop> {
	if request.method != "POST" || request.path.contains('?') {
		return None;
	}
	if request.path == "/scoped/generation/cancel" {
		let input: aidash_domain::generation::intent::Reference =
			serde_json::from_slice(request.body).ok()?;
		return Some(Stop::Generation(input.intent_id));
	}
	let id = request
		.path
		.strip_prefix("/scoped/execution/admissions/")?
		.strip_suffix("/control")?;
	let admission = Uuid::parse_str(id).ok()?;
	if admission.to_string() != id {
		return None;
	}
	let input: crate::apps::identity::serializers::peer_admission::RemoteExecutionControlInput =
		serde_json::from_slice(request.body).ok()?;
	if !matches!(
		input.action,
		aidash_domain::federation::execution::admission::RemoteExecutionControl::Cancel
	) {
		return None;
	}
	Some(Stop::Execution {
		admission,
		grant: input.grant_id,
	})
}

/// Proof for the sending side: durable cancellation, exact original Home Node,
/// target, intent/grant/admission and Tenant. A caller cannot supply an exemption.
pub(crate) async fn allows_closed_node_delivery_in(
	tx: &mut dyn TransactionExecutor,
	own: &aidash_domain::nodes::DeploymentId,
	sender: &Sender,
	receiver: &NodeAddress,
	request: &RequestDigest<'_>,
) -> Result<bool> {
	if sender.subject.is_some() {
		return Ok(false);
	}
	let Some(node) = &sender.node else {
		return Ok(false);
	};
	let allowed = match stop(request) {
		Some(Stop::Generation(id)) => {
			let binding: Option<serde_json::Value> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("binding"))
					.from(Alias::new("generation_remote_intents"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.and_where(
						Expr::col("tenant").eq(Expr::value(sender.tenant.as_str().to_owned())),
					)
					.and_where(Expr::col("cancelled"))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *tx)
			.await?;
			binding
				.map(|value| {
					serde_json::from_value::<aidash_domain::generation::intent::Intent>(value)
				})
				.transpose()?
				.is_some_and(|intent| {
					intent.id == id
						&& intent.source_tenant == sender.tenant.as_str()
						&& intent.home_node == own.node(node.clone()).to_string()
						&& intent.target_node == receiver.to_string()
				})
		}
		Some(Stop::Execution { admission, grant }) => {
			use snapshot::column as col;
			let id: Option<Uuid> = crate::database::native::query_scalar(
				&Query::select()
					.column((Alias::new("g"), Alias::new("id")))
					.from_as(Alias::new("authorization_remote_grants"), Alias::new("g"))
					.and_where(col("g", "id").eq(Expr::value(grant)))
					.and_where(col("g", "tenant").eq(Expr::value(sender.tenant.as_str())))
					.and_where(col("g", "node").eq(Expr::value(node.as_str())))
					.and_where(col("g", "node_id").eq(Expr::value(receiver.to_string())))
					.and_where(col("g", "revoked").eq(true))
					.and_where(Expr::exists(
						Query::select()
							.expr(Expr::value(1_i32))
							.from_as(
								Alias::new("authorization_remote_execution"),
								Alias::new("e"),
							)
							.and_where(col("e", "grant_id").eq(col("g", "id")))
							.and_where(col("e", "tenant").eq(col("g", "tenant")))
							.and_where(col("e", "task_id").eq(col("g", "task_id")))
							.and_where(col("e", "admission_id").eq(Expr::value(admission)))
							.to_owned(),
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *tx)
			.await?;
			id.is_some()
		}
		None => false,
	};
	Ok(allowed)
}

/// Inbound proof may be revalidated inside a key-continuity CAS transaction.
/// Signature/trust validation remains with the common inbound boundary. Only
/// Node-acting requests naming this receiver's immutable old obligation pass.
pub(crate) async fn allows_closed_node_request_in(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	receiver: &NodeAddress,
	sender: &NodeAddress,
	subject: &str,
	request: &RequestDigest<'_>,
) -> Result<bool> {
	if !subject.is_empty() {
		return Ok(false);
	}
	let id: Option<Uuid> = match stop(request) {
		Some(Stop::Generation(id)) => {
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("generation_requests"))
					.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
					.and_where(Expr::col("node").eq(Expr::value(receiver.node.as_str().to_owned())))
					.and_where(Expr::col("home_node").eq(Expr::value(sender.to_string())))
					.and_where(
						snapshot::json_text(Expr::col("foreign_intent").into(), "id")
							.eq(Expr::value(id.to_string())),
					)
					.and_where(
						snapshot::json_text(Expr::col("foreign_intent").into(), "home_node")
							.eq(Expr::value(sender.to_string())),
					)
					.and_where(
						snapshot::json_text(Expr::col("foreign_intent").into(), "target_node")
							.eq(Expr::value(receiver.to_string())),
					)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(tx)
			.await?
		}
		Some(Stop::Execution { admission, grant }) => {
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(Expr::col("id").eq(Expr::value(admission)))
					.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
					.and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
					.and_where(Expr::col("node").eq(Expr::value(receiver.node.as_str().to_owned())))
					.and_where(Expr::col("source_node").eq(Expr::value(sender.to_string())))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(tx)
			.await?
		}
		None => None,
	};
	Ok(id.is_some())
}

/// Fence reuse of a mutable Area/session until its captured writer has stopped.
pub(crate) async fn require_area_open(tx: &mut dyn TransactionExecutor, area: Uuid) -> Result<()> {
	let work = |kind: &str, table: &str| {
		Expr::col("kind").eq(Expr::value(kind)).and(
			Expr::col("work_id").in_subquery(
				Query::select()
					.column(Alias::new("id"))
					.from(Alias::new(table))
					.and_where(Expr::col("area_id").eq(Expr::value(area)))
					.to_owned(),
			),
		)
	};
	let pending: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(work("operation", "core_operations").or(work("python", "core_records")))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(tx)
	.await?;
	if pending.is_some() {
		return Err(Error::Conflict("Area cancellation is pending".into()));
	}
	Ok(())
}
async fn stop_operation(f: &Federation, job: &NodeCancellationJob) -> Result<bool> {
	use crate::apps::execution::repositories::operations::Operation;
	let scope = TenancyScope::tenant(&job.tenant)?;
	let operation: Operation = crate::database::native::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("core_operations"))
			.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.in_scope(&scope))
	.await?;
	aidash_application::capabilities::withdrawal::stop_captured_writer(
		&crate::bootstrap::operation_withdrawal_repository(&f.store, scope.clone()),
		operation.area_id,
		operation.id,
	)
	.await?;
	// The existing runtime housekeeping owns command ACK and its independent
	// allocation usage journal; a deletion job only observes their durable result.
	let state: (String, serde_json::Value, Option<String>) = crate::database::native::query_as(
		&Query::select()
			.columns([
				Alias::new("state"),
				Alias::new("result"),
				Alias::new("runner_instance"),
			])
			.from(Alias::new("core_operations"))
			.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["state", "result", "runner_instance"])
	.fetch_one(f.store.pool.in_scope(&scope))
	.await?;
	Ok(
		aidash_domain::capabilities::operations::withdrawal::safe_to_erase(
			&state.0,
			&state.1,
			state.2.as_deref(),
		),
	)
}
async fn stop_python(f: &Federation, job: &NodeCancellationJob) -> Result<bool> {
	let scope = TenancyScope::tenant(&job.tenant)?;
	let snapshot: crate::apps::execution::capabilities::serializers::records::Record =
		serde_json::from_value(job.binding.0.clone())?;
	let record: crate::apps::execution::capabilities::serializers::records::Record =
		crate::database::native::query_as(
			&Query::select()
				.column(reinhardt::query::ColumnRef::Asterisk)
				.from(Alias::new("core_records"))
				.and_where(Expr::col("id").eq(Expr::value(job.work_id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.in_scope(&scope))
		.await?;
	if record.data["session_id"] != snapshot.data["session_id"]
		|| record.data["instance"] != snapshot.data["instance"]
		|| record.data["operation_id"] != snapshot.data["operation_id"]
	{
		return Err(Error::Conflict("captured Python binding changed".into()));
	}
	if record.state == "reset" {
		return Ok(true);
	}
	if record.revision != snapshot.revision {
		return Err(Error::Conflict("captured Python revision changed".into()));
	}
	aidash_application::capabilities::python::stop_frozen(
		&crate::bootstrap::python_repository(&f.store),
		&super::capability_records::domain(snapshot),
		"owning_node_deleted",
	)
	.await
	.map_err(Into::into)
}
