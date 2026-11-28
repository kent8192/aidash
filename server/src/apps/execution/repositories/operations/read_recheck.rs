//! The 30 second Read Recheck of running commands and Live previews (ADR 0116).
use super::{pod_sessions, reconciliation::Repository};
use crate::{
	Error, Result as NativeResult,
	apps::{
		execution::repositories::live_previews, workspaces::repositories::scratch::default_space_id,
	},
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::{
		read_recheck::{Outcome, ReadRecheckRepository, Target},
		reconciliation::OperationReconciliationRepository as _,
	},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::preview::live::StopReason;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, BinOper, Expr, ExprTrait as _, IntoValue, OnConflict, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, types::PgBinOper,
};
use serde_json::json;
use std::collections::BTreeSet;
use tokio::task::JoinSet;
use uuid::Uuid;

async fn check_run(
	store: &Store,
	run_id: Uuid,
	identity: &SubjectIdentity,
	subjects: Vec<String>,
	kind: &str,
) -> NativeResult<()> {
	let mut access = Access::begin(store, identity).await?;
	access.subjects = subjects;
	let result = async {
		let mut check = access.rechecking(run_id);
		let run = check.recheck_run_reads(run_id).await?;
		// A list change acts like a withdrawal (ADR 0139).
		let tool = kind;
		if super::remote_tools::withdrawn(check.tx.as_mut(), run.id, tool).await? {
			return Err(Error::Forbidden);
		}
		// The Home's own Task is not on this Node for a Run executing here for another Home.
		if super::remote_tools::is_admitted(check.tx.as_mut(), run.id).await? {
			return Ok(());
		}
		check.task_read(run.task_id).await.map(|_| ())
	}
	.await;
	access.finish(result).await
}

async fn check_operation(repository: &Repository<'_>, id: Uuid) -> NativeResult<()> {
	let snapshot = repository.snapshot(id).await?;
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: snapshot.credential_id,
		tenant: snapshot.tenant.clone(),
		subject: snapshot.principal.clone(),
	};
	check_run(
		repository.store,
		snapshot.run_id,
		&identity,
		serde_json::from_value(snapshot.subjects.clone())?,
		&snapshot.kind,
	)
	.await
}

async fn check_stdio(store: &Store, run: Uuid) -> NativeResult<()> {
	let Some(session) = pod_sessions::stdio(store, run).await? else {
		return Ok(());
	};
	check_run(
		store,
		run,
		&session.identity(),
		session.subjects,
		"mcp_stdio",
	)
	.await
}

impl Repository<'_> {
	/// Durable targets stay eligible for maintenance until the Runner confirms
	/// the outcome. Runner timeouts must not delay the authorization pass.
	pub(crate) async fn maintain_read_recheck_targets(&self) -> Result<()> {
		const CONCURRENT_CALLS: usize = 4;
		let background = background_runs(self.store).await?;
		let probes = async {
			let mut jobs = JoinSet::new();
			for id in background {
				let store = self.store.clone();
				jobs.spawn(async move {
					let result: Result<()> = async {
						let snapshot = Repository { store: &store }.snapshot(id).await?;
						retire_ended_background(&store, &snapshot).await?;
						Ok(())
					}
					.await;
					if let Err(error) = result {
						tracing::warn!(%id, %error, "Background retirement pending");
					}
				});
				if jobs.len() == CONCURRENT_CALLS {
					join_maintenance_job(&mut jobs).await?;
				}
			}
			while join_maintenance_job(&mut jobs).await? {}
			Ok(())
		};
		let cleanup = async {
			let mut sessions = BTreeSet::new();
			for id in pod_sessions::python_targets(self.store).await? {
				match self.snapshot(id).await {
					Ok(snapshot) => {
						if let Some(space) = snapshot.input["scratch_space_id"].as_str() {
							sessions.insert((snapshot.tenant, snapshot.run_id, space.to_owned()));
						}
					}
					Err(error) => tracing::warn!(%id, %error, "Python session lookup pending"),
				}
			}
			for run in pod_sessions::stdio_targets(self.store).await? {
				match pod_sessions::stdio(self.store, run).await {
					Ok(Some(session)) => {
						sessions.insert((session.tenant, run, session.space));
					}
					Ok(None) => {}
					Err(error) => tracing::warn!(%run, %error, "Stdio session lookup pending"),
				}
			}
			let mut jobs = JoinSet::new();
			for (tenant, run, space) in sessions {
				let store = self.store.clone();
				jobs.spawn(async move {
					if let Err(error) =
						pod_sessions::stop_if_ended(&store, &tenant, run, &space).await
					{
						tracing::warn!(%run, %error, "terminal session cleanup pending");
					}
				});
				if jobs.len() == CONCURRENT_CALLS {
					join_maintenance_job(&mut jobs).await?;
				}
			}
			while join_maintenance_job(&mut jobs).await? {}
			Ok(())
		};
		let (probed, cleaned) = tokio::join!(probes, cleanup);
		probed.and(cleaned)
	}
}

async fn join_maintenance_job(jobs: &mut JoinSet<()>) -> Result<bool> {
	jobs.join_next()
		.await
		.transpose()
		.map(|job| job.is_some())
		.map_err(|error| aidash_application::Error::External(error.to_string()))
}

async fn retire_ended_background(
	store: &Store,
	snapshot: &aidash_domain::capabilities::operations::reconciliation::Snapshot,
) -> NativeResult<()> {
	if snapshot.input["runtime"] != "pod"
		|| snapshot.result["left_running"].is_null()
		|| snapshot.result["background_retired"] == true
	{
		return Ok(());
	}
	let Some(space) = snapshot.input["scratch_space_id"].as_str() else {
		return Ok(());
	};
	let Ok(runner) = crate::bootstrap::operation_runner(store) else {
		return Ok(());
	};
	let evidence = runner
		.background_ended(
			space,
			&snapshot.run_id.to_string(),
			&snapshot.id.to_string(),
			snapshot.result["pod_uid"].as_str(),
		)
		.await;
	if !matches!(evidence, Ok(true)) {
		return Ok(());
	}
	// Update the observed row only. Retain the Background id for shell_poll.
	crate::database::native::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value_expr(
				Alias::new("result"),
				Expr::col("result").binary(
					BinOper::PgOperator(PgBinOper::Concatenate),
					Expr::value(json!({"background_retired": true})),
				),
			)
			.and_where(Expr::col("id").eq(Expr::value(snapshot.id)))
			.and_where(Expr::col("tenant").eq(snapshot.tenant.clone()))
			.and_where(Expr::col("result").eq(Expr::value(snapshot.result.clone())))
			.to_string(PostgresQueryBuilder),
	)
	.execute(
		store
			.pool
			.in_scope(&TenancyScope::tenant(&snapshot.tenant)?),
	)
	.await?;
	Ok(())
}

async fn running_commands(store: &Store) -> NativeResult<Vec<Uuid>> {
	crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("state")).is_in(["prepared", "submitted", "running"]))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(store.pool.in_scope(&TenancyScope::Deployment(
		DeploymentScopeReason::CapabilityMaintenance,
	)))
	.await
}

pub(super) fn unrevoked(table: &str, run_column: &str) -> reinhardt::query::SimpleExpr {
	let revoked = Query::select()
		.expr(Expr::value(1_i32))
		.from_as(Alias::new("egress_run_revocations"), Alias::new("r"))
		.and_where(
			Expr::col((Alias::new("r"), Alias::new("run_id")))
				.equals((Alias::new(table), Alias::new(run_column))),
		)
		.to_owned();
	Expr::exists(revoked).not()
}

/// The Background-target query: one ended Pod command per Run that left Background processes
/// running, for Runs the Read Recheck has not refused.
fn background_runs_query() -> String {
	Query::select()
		.distinct_on([Alias::new("run_id")])
		.column(Alias::new("id"))
		.from(Alias::new("core_operations"))
		.and_where(Expr::col(Alias::new("state")).is_not_in(["prepared", "submitted", "running"]))
		.and_where(Expr::cust("input->>'runtime' = 'pod'"))
		.and_where(Expr::cust("result->>'left_running' IS NOT NULL"))
		.and_where(
			Expr::col("result")
				.binary(
					BinOper::PgOperator(PgBinOper::Contains),
					Expr::value(json!({"background_retired": true})),
				)
				.not(),
		)
		.and_where(unrevoked("core_operations", "run_id"))
		.order_by(Alias::new("run_id"), Order::Asc)
		.order_by(Alias::new("updated_at"), Order::Desc)
		.to_string(PostgresQueryBuilder)
}

/// One ended Pod command per Run that left Background processes running, for
/// Runs not yet refused. Background processes outlive their command and the
/// Run, and use its credential, so their Run's reads are rechecked like a
/// running command's (ADR 0136). A refusal marks the Run, which ends the target.
async fn background_runs(store: &Store) -> NativeResult<Vec<Uuid>> {
	crate::database::native::query_scalar(&background_runs_query())
		.scalar_all(store.pool.in_scope(&TenancyScope::Deployment(
			DeploymentScopeReason::CapabilityMaintenance,
		)))
		.await
}

/// Persist the denial before contacting the Runner: traffic must remain refused
/// even when the Runner is unavailable and stopping has to be retried.
async fn stop_pod_run(store: &Store, tenant: &str, run_id: Uuid) -> Result<()> {
	let mut tx = store.pool.begin(&TenancyScope::tenant(tenant)?).await?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new("egress_run_revocations"))
		.columns(["tenant", "run_id", "revoked_at", "reason"].map(Alias::new))
		.values_panic([
			tenant.to_owned().into_value(),
			run_id.into_value(),
			chrono::Utc::now().into_value(),
			"read_refused".to_owned().into_value(),
		])
		.on_conflict(
			OnConflict::column(Alias::new("run_id"))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(Error::from)?;
	tx.commit().await?;

	let runner =
		crate::bootstrap::operation_runner(store).map_err(aidash_application::Error::from)?;
	stop_pending_run_with_runner(store, tenant, run_id, &runner).await
}

/// Retry an already-recorded Run stop, acknowledging it only after the Runner confirms it.
pub(crate) async fn stop_pending_run_with_runner(
	store: &Store,
	tenant: &str,
	run_id: Uuid,
	runner: &dyn aidash_application::ports::capabilities::execution_pod::ExecutionPodRuntime,
) -> Result<()> {
	use crate::apps::execution::models::Run;
	let mut tx = store.pool.begin(&TenancyScope::tenant(tenant)?).await?;
	let run = Run::objects()
		.filter(Run::field_tenant().eq(tenant.to_owned()))
		.filter(Run::field_id().eq(run_id))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)
		.map_err(Error::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("Run".into()))?;
	let space =
		if aidash_domain::nodes::is_home(&run.home_node, store.deployment.as_ref(), &run.node) {
			let sql = Query::select()
				.column(Alias::new("scratch_space_id"))
				.from(Alias::new("threads"))
				.and_where(Expr::col("tenant").eq(tenant.to_owned()))
				.and_where(Expr::col("id").eq(Expr::value(run.thread_id)))
				.and_where(Expr::col("node").eq(run.node.clone()))
				.to_string(PostgresQueryBuilder);
			crate::database::native::query_scalar::<Uuid>(&sql)
				.scalar_optional(&mut *tx)
				.await?
				.ok_or_else(|| Error::NotFound("Run Scratch Space".into()))?
		} else {
			default_space_id(&mut *tx, tenant, &run.node).await?
		};
	tx.commit().await?;
	runner
		.stop_run(&space.to_string(), &run_id.to_string())
		.await
		.map_err(|error| {
			aidash_application::Error::External(format!("Run stop failed: {error}"))
		})?;
	// If this update fails, the next sweep repeats the idempotent Runner stop.
	let mut tx = store.pool.begin(&TenancyScope::tenant(tenant)?).await?;
	let (sql, values) = Query::update()
		.table(Alias::new("egress_run_revocations"))
		.value(Alias::new("stopped_at"), chrono::Utc::now())
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
		.and_where(Expr::col("stopped_at").is_null())
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(Error::from)?;
	tx.commit().await?;
	Ok(())
}

#[async_trait]
impl ReadRecheckRepository for Repository<'_> {
	async fn targets(&self) -> Result<Vec<Target>> {
		let operations: BTreeSet<_> = running_commands(self.store)
			.await?
			.into_iter()
			.chain(background_runs(self.store).await?)
			.chain(pod_sessions::python_targets(self.store).await?)
			.collect();
		let mut targets: Vec<_> = operations.into_iter().map(Target::Operation).collect();
		targets.extend(
			pod_sessions::stdio_targets(self.store)
				.await?
				.into_iter()
				.map(Target::StdioSession),
		);
		targets.extend(
			live_previews::serving(self.store)
				.await?
				.into_iter()
				.map(Target::LivePreview),
		);
		let pending: Vec<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("run_id"))
				.from(Alias::new("egress_run_revocations"))
				.and_where(Expr::col("stopped_at").is_null())
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(self.store.pool.in_scope(&TenancyScope::Deployment(
			DeploymentScopeReason::CapabilityMaintenance,
		)))
		.await?;
		targets.extend(pending.into_iter().map(Target::RevokedRun));
		Ok(targets)
	}
	async fn check(&self, target: Target) -> Result<Outcome> {
		let checked = match target {
			Target::Operation(id) => check_operation(self, id).await.map(|()| true),
			Target::StdioSession(id) => check_stdio(self.store, id).await.map(|()| true),
			Target::LivePreview(id) => live_previews::read_recheck(self.store, id).await,
			Target::RevokedRun(_) => return Ok(Outcome::Refused),
		};
		match checked {
			Ok(true) => Ok(Outcome::Allowed),
			Ok(false) => Ok(Outcome::Unconfirmed),
			Err(
				Error::Forbidden
				| Error::Unauthorized
				| Error::CredentialForbidden(_)
				| Error::AccountSuspended { .. }
				| Error::NotFound(_),
			) => Ok(Outcome::Refused),
			// A Node that does not answer refuses nothing (#906, ADR 0117).
			Err(Error::NodeUnreachable { .. }) => Ok(Outcome::Unconfirmed),
			Err(error) => Err(error.into()),
		}
	}
	async fn stop_refused(&self, target: Target) -> Result<()> {
		match target {
			Target::RevokedRun(id) => {
				let tenant: Option<String> = crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("tenant"))
						.from(Alias::new("egress_run_revocations"))
						.and_where(Expr::col("run_id").eq(Expr::value(id)))
						.and_where(Expr::col("stopped_at").is_null())
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(self.store.pool.in_scope(&TenancyScope::Deployment(
					DeploymentScopeReason::CapabilityMaintenance,
				)))
				.await?;
				match tenant {
					Some(tenant) => stop_pod_run(self.store, &tenant, id).await,
					None => Ok(()),
				}
			}
			Target::StdioSession(id) => {
				if let Some(session) = pod_sessions::stdio(self.store, id).await? {
					stop_pod_run(self.store, &session.tenant, id).await?;
				}
				Ok(())
			}
			Target::Operation(id) => {
				let snapshot = self.snapshot(id).await?;
				let pod_stop = stop_pod_run(self.store, &snapshot.tenant, snapshot.run_id).await;
				let legacy_stop = self.withdraw(&snapshot).await;
				pod_stop.and(legacy_stop)
			}
			Target::LivePreview(id) => {
				use crate::apps::execution::capabilities::models::CoreLivePreviews;
				let mut tx = self
					.store
					.pool
					.begin(&TenancyScope::Deployment(
						DeploymentScopeReason::CapabilityMaintenance,
					))
					.await?;
				let preview = CoreLivePreviews::objects()
					.filter(CoreLivePreviews::field_id().eq(id))
					.all_with_executor(&mut *tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)
					.map_err(Error::from)?
					.pop();
				tx.commit().await?;
				let Some(preview) = preview else {
					return Ok(());
				};
				let pod_stop = stop_pod_run(self.store, &preview.tenant, preview.run_id).await;
				let legacy_stop =
					live_previews::stop_serving(self.store, id, StopReason::RunAuthorityRevoked)
						.await
						.map_err(Into::into);
				pod_stop.and(legacy_stop)
			}
		}
	}
	async fn withhold(&self, preview: Uuid) -> Result<Option<DateTime<Utc>>> {
		live_previews::set_unconfirmed(self.store, preview, true)
			.await
			.map_err(Into::into)
	}
	async fn serve_again(&self, preview: Uuid) -> Result<()> {
		live_previews::set_unconfirmed(self.store, preview, false)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn stop_unconfirmed(&self, preview: Uuid) -> Result<()> {
		live_previews::stop_serving(self.store, preview, StopReason::NodeUnreachable)
			.await
			.map_err(Into::into)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_refusal_ends_only_its_own_runs_background_target() {
		let sql = background_runs_query();
		assert!(
			sql.contains(r#"NOT EXISTS (SELECT 1 FROM "egress_run_revocations" AS "r" WHERE "r"."run_id" = "core_operations"."run_id")"#),
			"{sql}"
		);
	}
}
