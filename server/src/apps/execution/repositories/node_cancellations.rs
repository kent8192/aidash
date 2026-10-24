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
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use uuid::Uuid;

pub(crate) mod home_commands;

const BATCH: usize = 32;

/// Called immediately after NodeDeletion insertion, under its policy/capacity
/// locks. The snapshot survives restore; subsequent work is never selected.
pub(crate) async fn record(
	f: &Federation,
	tx: &mut dyn TransactionExecutor,
	deletion: &crate::apps::identity::models::NodeDeletion,
) -> Result<()> {
	let owner =
		|sql: &str, values: Vec<SimpleExpr>| SimpleExpr::CustomWithExpr(format!("({sql})"), values);
	let run_owner = owner(
		"node = ? OR EXISTS(SELECT 1 FROM workspaces w WHERE w.id=runs.workspace_id AND w.tenant=runs.tenant AND w.node=? AND runs.home_node=?) OR EXISTS(SELECT 1 FROM core_runs cr JOIN core_areas a ON a.id=cr.area_id AND a.tenant=cr.tenant JOIN workspaces w ON w.id=a.workspace_id AND w.tenant=a.tenant WHERE cr.run_id=runs.id AND cr.tenant=runs.tenant AND w.node=? AND a.home_node=?)",
		vec![
			Expr::value(deletion.node.clone()).into(),
			Expr::value(deletion.node.clone()).into(),
			Expr::value(format!("{}/{}", f.config.deployment, deletion.node)).into(),
			Expr::value(deletion.node.clone()).into(),
			Expr::value(format!("{}/{}", f.config.deployment, deletion.node)).into(),
		],
	);
	let capability_owner = |table: &str| {
		owner(
			&format!(
				"EXISTS(SELECT 1 FROM core_areas a JOIN workspaces w ON w.id=a.workspace_id AND w.tenant=a.tenant WHERE a.id={table}.area_id AND a.tenant={table}.tenant AND w.node=? AND a.home_node=?) OR EXISTS(SELECT 1 FROM runs r WHERE r.tenant={table}.tenant AND r.node=? AND r.id={})",
				if table == "core_operations" {
					"core_operations.run_id"
				} else {
					"(SELECT o.run_id FROM core_operations o WHERE o.id::text=core_records.data->>'operation_id' AND o.tenant=core_records.tenant)"
				}
			),
			vec![
				Expr::value(deletion.node.clone()).into(),
				Expr::value(format!("{}/{}", f.config.deployment, deletion.node)).into(),
				Expr::value(deletion.node.clone()).into(),
			],
		)
	};
	for (kind, table, predicate, pending) in [
		(
			"run",
			"runs",
			run_owner,
			"phase NOT IN ('COMPLETED','FAILED','CANCELLED') OR lease_owner IS NOT NULL",
		),
		(
			"admission",
			"authorization_remote_admissions",
			Expr::col("node").eq(Expr::value(deletion.node.clone())),
			"NOT cancelled",
		),
		(
			"generation",
			"generation_requests",
			Expr::col("node").eq(Expr::value(deletion.node.clone())),
			"status IN ('PENDING_APPROVAL','QUEUED','ACTIVE') OR NOT quota_released",
		),
		(
			"grant",
			"authorization_remote_grants",
			owner(
				"EXISTS (SELECT 1 FROM workspaces w WHERE w.id=authorization_remote_grants.workspace_id AND w.tenant=authorization_remote_grants.tenant AND w.node=?) OR node_id=?",
				vec![
					Expr::value(deletion.node.clone()).into(),
					Expr::value(format!("{}/{}", f.config.deployment, deletion.node)).into(),
				],
			),
			"TRUE",
		),
		(
			"intent",
			"generation_remote_intents",
			owner(
				"EXISTS (SELECT 1 FROM tasks t JOIN workspaces w ON w.id=t.workspace_id AND w.tenant=t.tenant WHERE t.id=generation_remote_intents.task_id AND t.tenant=generation_remote_intents.tenant AND w.node=?)",
				vec![Expr::value(deletion.node.clone()).into()],
			),
			"NOT cancel_delivered",
		),
		(
			"task",
			"tasks",
			owner(
				"EXISTS (SELECT 1 FROM workspaces w WHERE w.id=tasks.workspace_id AND w.tenant=tasks.tenant AND w.node=?) OR EXISTS (SELECT 1 FROM authorization_remote_execution e JOIN authorization_remote_grants g ON g.id=e.grant_id AND g.tenant=e.tenant WHERE e.task_id=tasks.id AND e.tenant=tasks.tenant AND e.task_revision=tasks.revision AND g.node_id=?)",
				vec![
					Expr::value(deletion.node.clone()).into(),
					Expr::value(format!("{}/{}", f.config.deployment, deletion.node)).into(),
				],
			),
			"status NOT IN ('COMPLETED','CANCELLED','ABANDONED')",
		),
		(
			"operation",
			"core_operations",
			capability_owner("core_operations"),
			"NOT ((state IN ('completed','cancelled','failed','withdrawn') AND ((COALESCE(result->'termination_confirmed'='true'::jsonb,false) AND COALESCE(result->'runner_acknowledged'='true'::jsonb,false)) OR (runner_instance IS NULL AND COALESCE(result->'effects_may_have_occurred'='false'::jsonb,false)))) OR (state='uncertain' AND COALESCE(result->'termination_confirmed'='true'::jsonb,false) AND COALESCE(result->'runner_acknowledged'='true'::jsonb,false)))",
		),
		(
			"python",
			"core_records",
			capability_owner("core_records"),
			"kind='python_session' AND state='frozen'",
		),
	] {
		let select = Query::select()
			.expr(owner(
				"(? || CAST(id AS text))",
				vec![Expr::value(format!("{}:{kind}:", deletion.id)).into()],
			))
			.expr(Expr::value(deletion.id))
			.expr(Expr::value(deletion.tenant.clone()))
			.expr(Expr::value(deletion.node.clone()))
			.expr(Expr::value(kind))
			.column(Alias::new("id"))
			.expr(if kind == "python" {
				Expr::cust("to_jsonb(core_records.*)")
			} else {
				Expr::value(serde_json::Value::Null).into()
			})
			.expr(Expr::current_timestamp())
			.from(Alias::new(table))
			.and_where(Expr::col("tenant").eq(Expr::value(deletion.tenant.clone())))
			.and_where(predicate)
			.and_where(Expr::cust(format!("({pending})")))
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
			.and_where(SimpleExpr::CustomWithExpr("id IN (SELECT work_id FROM node_cancellation_jobs WHERE deletion_id = ? AND kind='run')".into(), vec![Expr::value(deletion.id).into()]))
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
pub(crate) async fn require_admission_open(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
) -> Result<()> {
	require_work_open(tx, "admission", id).await
}
pub(crate) async fn require_run_open(tx: &mut dyn TransactionExecutor, run: Uuid) -> Result<()> {
	require_work_open(tx, "run", run).await
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
					Expr::cust("CLOCK_TIMESTAMP() + INTERVAL '30 seconds'"),
				)
				.and_where(Expr::col("id").eq(Expr::value(job.id.clone())))
				.and_where(Expr::cust("retry_at <= CLOCK_TIMESTAMP()"))
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
	f.store
		.cancel_owned_execution(&scope, &owned, token)
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
			.and_where(SimpleExpr::CustomWithExpr(
				"work_id IN (SELECT id FROM authorization_remote_grants WHERE task_id = ?)".into(),
				vec![Expr::value(job.work_id).into()],
			))
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
	if !matches!(
		task.status,
		TaskStatus::Completed | TaskStatus::Cancelled | TaskStatus::Abandoned
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
pub(crate) async fn allows_closed_node_delivery(
	f: &Federation,
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
	let scope = TenancyScope::tenant(sender.tenant.as_str())?;
	let mut tx = f.store.pool.begin(&scope).await?;
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
						&& intent.home_node == f.config.deployment.node(node.clone()).to_string()
						&& intent.target_node == receiver.to_string()
				})
		}
		Some(Stop::Execution { admission, grant }) => {
			let id:Option<Uuid>=crate::database::native::query_scalar(&Query::select().column((Alias::new("g"),Alias::new("id")))
                .from_as(Alias::new("authorization_remote_grants"),Alias::new("g"))
                .and_where(SimpleExpr::CustomWithExpr(
                    "g.id=? AND g.tenant=? AND g.node=? AND g.node_id=? AND g.revoked AND EXISTS(SELECT 1 FROM authorization_remote_execution e WHERE e.grant_id=g.id AND e.tenant=g.tenant AND e.task_id=g.task_id AND e.admission_id=?)".into(),
                    vec![Expr::value(grant).into(),Expr::value(sender.tenant.as_str().to_owned()).into(),Expr::value(node.as_str().to_owned()).into(),Expr::value(receiver.to_string()).into(),Expr::value(admission).into()]))
                .to_string(PostgresQueryBuilder)).scalar_optional(&mut *tx).await?;
			id.is_some()
		}
		None => false,
	};
	tx.commit().await?;
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
	let id:Option<Uuid>=match stop(request) {
        Some(Stop::Generation(id))=>crate::database::native::query_scalar(&Query::select().column(Alias::new("id"))
            .from(Alias::new("generation_requests"))
            .and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
            .and_where(Expr::col("node").eq(Expr::value(receiver.node.as_str().to_owned())))
            .and_where(Expr::col("home_node").eq(Expr::value(sender.to_string())))
            .and_where(SimpleExpr::CustomWithExpr("foreign_intent->>'id'=? AND foreign_intent->>'home_node'=? AND foreign_intent->>'target_node'=?".into(),
                vec![Expr::value(id.to_string()).into(),Expr::value(sender.to_string()).into(),Expr::value(receiver.to_string()).into()]))
            .to_string(PostgresQueryBuilder)).scalar_optional(tx).await?,
        Some(Stop::Execution{admission,grant})=>crate::database::native::query_scalar(&Query::select().column(Alias::new("id"))
            .from(Alias::new("authorization_remote_admissions"))
            .and_where(Expr::col("id").eq(Expr::value(admission)))
            .and_where(Expr::col("grant_id").eq(Expr::value(grant)))
            .and_where(Expr::col("tenant").eq(Expr::value(tenant.to_owned())))
            .and_where(Expr::col("node").eq(Expr::value(receiver.node.as_str().to_owned())))
            .and_where(Expr::col("source_node").eq(Expr::value(sender.to_string())))
            .to_string(PostgresQueryBuilder)).scalar_optional(tx).await?,
        None=>None,
    };
	Ok(id.is_some())
}

/// Fence reuse of a mutable Area/session until its captured writer has stopped.
pub(crate) async fn require_area_open(tx: &mut dyn TransactionExecutor, area: Uuid) -> Result<()> {
	let pending: Option<String> = crate::database::native::query_scalar(&Query::select()
        .column(Alias::new("id")).from(Alias::new("node_cancellation_jobs"))
        .and_where(SimpleExpr::CustomWithExpr("(kind='operation' AND work_id IN(SELECT id FROM core_operations WHERE area_id=?)) OR (kind='python' AND work_id IN(SELECT id FROM core_records WHERE area_id=?))".into(),vec![Expr::value(area).into(),Expr::value(area).into()]))
        .limit(1).to_string(PostgresQueryBuilder)).scalar_optional(tx).await?;
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
