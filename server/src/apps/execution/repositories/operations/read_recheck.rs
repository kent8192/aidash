//! The 30 second Read Recheck of running commands and Live previews (ADR 0116).
use super::reconciliation::Repository;
use crate::{
	Error, Result as NativeResult,
	apps::execution::repositories::live_previews,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::{
		execution_pod::{
			CommandResult, ExecutionPodError, ExecutionPodErrorCode, ExecutionPodRuntime as _,
		},
		read_recheck::{Outcome, ReadRecheckRepository, Target},
		reconciliation::OperationReconciliationRepository as _,
	},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::preview::live::StopReason;
use async_trait::async_trait;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, IntoValue, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _,
};
use uuid::Uuid;

async fn check_operation(repository: &Repository<'_>, id: Uuid) -> NativeResult<()> {
	let snapshot = repository.snapshot(id).await?;
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: snapshot.credential_id,
		tenant: snapshot.tenant.clone(),
		subject: snapshot.principal.clone(),
	};
	let mut access = Access::begin(repository.store, &identity).await?;
	access.subjects = serde_json::from_value(snapshot.subjects.clone())?;
	let result = async {
		let mut check = access.rechecking(snapshot.run_id);
		let run = check.recheck_run_reads(snapshot.run_id).await?;
		check.task_read(run.task_id).await.map(|_| ())
	}
	.await;
	access.finish(result).await
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

/// Ended Pod commands that left Background processes not yet seen to end, of Runs the
/// Read Recheck has not refused, newest first within each Run.
fn background_candidates() -> String {
	// Correlated with the outer row: only this Run's refusal ends its targets.
	let revoked = Query::select()
		.expr(Expr::value(1_i32))
		.from_as(Alias::new("egress_run_revocations"), Alias::new("r"))
		.and_where(
			Expr::col((Alias::new("r"), Alias::new("run_id")))
				.equals((Alias::new("core_operations"), Alias::new("run_id"))),
		)
		.to_owned();
	Query::select()
		.column(Alias::new("id"))
		.column(Alias::new("run_id"))
		.column(Alias::new("state"))
		.column(Alias::new("result"))
		.from(Alias::new("core_operations"))
		.and_where(Expr::col(Alias::new("state")).is_not_in(["prepared", "submitted", "running"]))
		.and_where(Expr::cust("input->>'runtime' = 'pod'"))
		.and_where(Expr::cust("result->>'left_running' IS NOT NULL"))
		.and_where(Expr::cust("result->>'left_ended' IS NULL"))
		.and_where(Expr::exists(revoked).not())
		.order_by(Alias::new("run_id"), Order::Asc)
		.order_by(Alias::new("updated_at"), Order::Desc)
		.to_string(PostgresQueryBuilder)
}

/// Whether the Runner shows that the Background processes a command left have ended. Only
/// positive evidence counts: an unreachable Runner, or a process whose stop is still owed,
/// keeps its Run a target.
fn left_ended(answer: &std::result::Result<CommandResult, ExecutionPodError>) -> bool {
	match answer {
		Ok(result) => result.ended == Some(true),
		// The Runner prunes a record only once it has settled.
		Err(ExecutionPodError::Remote {
			code: ExecutionPodErrorCode::CommandNotFound,
			..
		}) => true,
		Err(_) => false,
	}
}

/// Per Run, the newest candidate whose left processes may still run is its target. Candidates
/// come as (command, Run, left process id), newest first within each Run; `ended` asks the
/// Runner, and is not asked again once a Run has its target. Returns the targets and the
/// commands shown ended.
async fn decide_background<F, Fut>(
	candidates: Vec<(Uuid, Uuid, Option<String>)>,
	mut ended: F,
) -> (Vec<Uuid>, Vec<Uuid>)
where
	F: FnMut(String) -> Fut,
	Fut: std::future::Future<Output = bool>,
{
	let mut decided = std::collections::BTreeSet::new();
	let (mut targets, mut finished) = (Vec::new(), Vec::new());
	for (id, run, left) in candidates {
		if decided.contains(&run) {
			continue;
		}
		let shown_ended = match left {
			Some(left) => ended(left).await,
			None => false,
		};
		if shown_ended {
			finished.push(id);
		} else {
			decided.insert(run);
			targets.push(id);
		}
	}
	(targets, finished)
}

/// One ended Pod command per Run whose left Background processes may still run, for Runs not
/// yet refused. Background processes outlive their command and the Run, and use its
/// credential, so their Run's reads are rechecked like a running command's (ADR 0136). A
/// refusal marks the Run, which ends its targets; a command whose left processes the Runner
/// shows ended is marked `left_ended` and is never a target again, so the targets do not grow
/// with every command that once left a process.
async fn background_runs(store: &Store) -> NativeResult<Vec<Uuid>> {
	let scope = TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
	let rows: Vec<(Uuid, Uuid, String, serde_json::Value)> =
		crate::database::native::query_as(&background_candidates())
			.fetch_all(store.pool.in_scope(&scope))
			.await?;
	if rows.is_empty() {
		return Ok(Vec::new());
	}
	// Without a Runner nothing is shown to have ended, so every Run stays a target.
	let runner = crate::bootstrap::operation_runner(store).ok();
	let candidates = rows
		.iter()
		.map(|(id, run, _, result)| {
			let left = result["left_running"].as_str().map(str::to_owned);
			(*id, *run, left.filter(|_| runner.is_some()))
		})
		.collect();
	let runner = runner.as_ref();
	let (targets, finished) = decide_background(candidates, |left| async move {
		match runner {
			Some(runner) => left_ended(&runner.result(&left).await),
			None => false,
		}
	})
	.await;
	for (id, _, state, result) in rows.into_iter().filter(|row| finished.contains(&row.0)) {
		let mut result = result;
		result["left_ended"] = serde_json::Value::Bool(true);
		// Terminal rows keep their state; the state check only guards against a row
		// that is somehow live again.
		crate::database::native::query(
			&Query::update()
				.table(Alias::new("core_operations"))
				.value(Alias::new("result"), result)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("state")).eq(Expr::value(state)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(store.pool.in_scope(&scope))
		.await?;
	}
	Ok(targets)
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
	use crate::apps::federation::peer::models::AuthorizationRemoteAdmission;
	use crate::apps::identity::models::AuthorizationCredential;
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
				.from(Alias::new("workspaces"))
				.and_where(Expr::col("tenant").eq(tenant.to_owned()))
				.and_where(Expr::col("id").eq(Expr::value(run.workspace_id)))
				.and_where(Expr::col("node").eq(run.node.clone()))
				.to_string(PostgresQueryBuilder);
			crate::database::native::query_scalar::<Uuid>(&sql)
				.scalar_optional(&mut *tx)
				.await?
				.ok_or_else(|| Error::NotFound("Run Scratch Space".into()))?
		} else {
			let admission = AuthorizationRemoteAdmission::objects()
				.filter(AuthorizationRemoteAdmission::field_tenant().eq(tenant.to_owned()))
				.filter(AuthorizationRemoteAdmission::field_id().eq(run_id))
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)
				.map_err(Error::from)?
				.pop()
				.ok_or_else(|| Error::NotFound("Run admission".into()))?;
			let person = if admission.credential_id.is_nil() {
				admission
					.subject_chain
					.first()
					.cloned()
					.ok_or_else(|| Error::NotFound("Run subject".into()))?
			} else {
				AuthorizationCredential::objects()
					.filter(AuthorizationCredential::field_tenant().eq(tenant.to_owned()))
					.filter(AuthorizationCredential::field_id().eq(admission.credential_id))
					.all_with_executor(&mut *tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)
					.map_err(Error::from)?
					.pop()
					.ok_or_else(|| Error::NotFound("Run credential".into()))?
					.subject
			};
			remote_space_id(tenant, &run.node, &run.home_node, run.workspace_id, &person)
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

// Same deterministic identity as remote Space allocation. It must be resolvable
// after authorization has been revoked, without performing a new admission.
fn remote_space_id(tenant: &str, node: &str, home: &str, workspace: Uuid, person: &str) -> Uuid {
	use sha2::{Digest, Sha256};
	let mut hash = Sha256::new();
	hash.update(b"aidash:remote-scratch-space:v1");
	for value in [tenant, node, home, person] {
		hash.update((value.len() as u64).to_be_bytes());
		hash.update(value.as_bytes());
	}
	hash.update(workspace.as_bytes());
	let digest = hash.finalize();
	let mut bytes = [0_u8; 16];
	bytes.copy_from_slice(&digest[..16]);
	bytes[6] = (bytes[6] & 0x0f) | 0x80;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	Uuid::from_bytes(bytes)
}

#[async_trait]
impl ReadRecheckRepository for Repository<'_> {
	async fn targets(&self) -> Result<Vec<Target>> {
		let mut targets: Vec<Target> = running_commands(self.store)
			.await?
			.into_iter()
			.chain(background_runs(self.store).await?)
			.map(Target::Operation)
			.collect();
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
			Target::LivePreview(id) => live_previews::read_recheck(self.store, id).await,
			Target::RevokedRun(_) => return Ok(Outcome::Refused),
		};
		match checked {
			Ok(true) => Ok(Outcome::Allowed),
			Ok(false) => Ok(Outcome::Unconfirmed),
			Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
				Ok(Outcome::Refused)
			}
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
	fn a_refusal_ends_only_its_own_runs_background_targets() {
		let sql = background_candidates();
		assert!(
			sql.contains(r#"NOT EXISTS (SELECT 1 FROM "egress_run_revocations" AS "r" WHERE "r"."run_id" = "core_operations"."run_id")"#),
			"{sql}"
		);
		assert!(sql.contains("result->>'left_ended' IS NULL"), "{sql}");
	}

	fn result(ended: Option<bool>) -> CommandResult {
		serde_json::from_value(serde_json::json!({
			"operation_id": "op.left", "status": "stopped", "scratch_space_id": null, "run_id": null,
			"background": true, "outcome": null, "exit_code": null, "stdout": null, "stderr": null,
			"truncated": null, "error_code": null, "message": null, "python_result": null,
			"left_running": null, "ended": ended,
		}))
		.unwrap()
	}

	#[test]
	fn only_the_runners_evidence_ends_a_background_target() {
		assert!(left_ended(&Ok(result(Some(true)))));
		assert!(!left_ended(&Ok(result(Some(false)))));
		assert!(!left_ended(&Ok(result(None))));
		assert!(left_ended(&Err(ExecutionPodError::Remote {
			code: ExecutionPodErrorCode::CommandNotFound,
			message: "Command does not exist".into(),
		})));
		assert!(!left_ended(&Err(ExecutionPodError::Transport(
			aidash_application::Error::External("runner offline".into())
		))));
	}

	#[tokio::test]
	async fn each_run_keeps_its_newest_command_whose_left_processes_may_run() {
		let id = |n: u128| Uuid::from_u128(n);
		let (a, b) = (id(100), id(200));
		let candidates = vec![
			(id(1), a, Some("1.left".to_owned())),
			(id(2), a, Some("2.left".to_owned())),
			(id(3), a, Some("3.left".to_owned())),
			(id(4), b, Some("4.left".to_owned())),
			(id(5), b, None),
		];
		let asked = std::sync::Mutex::new(Vec::new());
		let (targets, finished) = decide_background(candidates, |left| {
			asked.lock().unwrap().push(left.clone());
			async move { matches!(left.as_str(), "1.left" | "4.left") }
		})
		.await;
		// Run a: 1 ended, 2 is the target, 3 is not asked. Run b: 4 ended; 5 cannot be asked.
		assert_eq!(targets, [id(2), id(5)]);
		assert_eq!(finished, [id(1), id(4)]);
		assert_eq!(*asked.lock().unwrap(), ["1.left", "2.left", "4.left"]);
	}

	#[test]
	fn remote_run_space_identity_survives_revocation() {
		let workspace = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap();
		assert_eq!(
			remote_space_id("acme", "node", "aidash://home/node", workspace, "alice"),
			Uuid::parse_str("9d8ef6c4-1f97-8300-ac32-d5431655fa0c").unwrap()
		);
		assert_ne!(
			remote_space_id("acme", "node", "aidash://home/node", workspace, "bob"),
			remote_space_id("acme", "node", "aidash://home/node", workspace, "alice")
		);
	}
}
