//! Agent tools in the Execution Pod of a Run's Scratch Space (#745): the Pod a
//! tool call reaches, and the capability worker's view of Pod operations.
use super::{Operation, reconciliation::Repository};
use crate::{
	Error as NativeError, Result as NativeResult,
	apps::{
		execution::{
			repositories::scope_egress::ScratchAuthority,
			services::execution_pod_command::pod_settings,
		},
		workspaces::repositories::scratch,
	},
	authorization::access::Access,
	config::ScratchOperatorPolicy,
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::{
		execution_pod::{EgressCredential, ExecutionPodRuntime},
		pod_operations::{PodContext, PodOperation, PodOperationRepository},
	},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::{RunMetadata, scratch::ScratchSpace};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
/// Selects operations whose recorded input says they run in a Pod.
const POD_ROWS: &str = "input->>'runtime' = 'pod'";

/// The operator policy Pod settings and Run credentials come from. A running
/// server binds it with its Federation; a process without it cannot run tools.
fn policy(store: &Store) -> NativeResult<ScratchOperatorPolicy> {
	store
		.in_process
		.get()
		.map(|bound| bound.config.scratch_policy.clone())
		.ok_or_else(|| {
			NativeError::Conflict(
				"RUNTIME_UNAVAILABLE: this process has no Scratch Space policy for Execution Pods"
					.into(),
			)
		})
}

/// Whether the Read Recheck has marked this Run revoked (ADR 0136). A marked
/// Run starts nothing new in the Pod, so a call after the stop cannot slip in.
async fn run_revoked(
	executor: impl crate::database::native::Executor,
	tenant: &str,
	run: Uuid,
) -> NativeResult<bool> {
	let sql = Query::select()
		.column(Alias::new("run_id"))
		.from(Alias::new("egress_run_revocations"))
		.and_where(Expr::col("tenant").eq(tenant.to_owned()))
		.and_where(Expr::col("run_id").eq(Expr::value(run)))
		.to_string(PostgresQueryBuilder);
	Ok(crate::database::native::query_scalar::<Uuid>(&sql)
		.scalar_optional(executor)
		.await?
		.is_some())
}

fn context(
	store: &Store,
	policy: &ScratchOperatorPolicy,
	space: &ScratchSpace,
	tenant: &str,
	run: Uuid,
) -> NativeResult<PodContext> {
	let image = &store
		.capabilities
		.0
		.runner
		.as_ref()
		.ok_or_else(|| NativeError::Conflict("RUNTIME_UNAVAILABLE: no isolated runner".into()))?
		.image;
	let settings = pod_settings(space, tenant, policy, image)?;
	let credential =
		ScratchAuthority::new(store.clone(), policy.clone()).credential(tenant, space.id, run)?;
	let runtime: Arc<dyn ExecutionPodRuntime> =
		Arc::new(crate::bootstrap::operation_runner(store)?);
	Ok(PodContext {
		runtime,
		settings,
		credential: Some(EgressCredential(credential)),
	})
}

/// The Pod a Run's tools reach, from the Scratch Space bound to the Run as it
/// is now. None when this Deployment has no Runner.
pub(crate) async fn tool_context(
	store: &Store,
	access: &mut Access,
	run: &RunMetadata,
) -> Result<Option<PodContext>> {
	context_for_tool(store, access, run, false).await
}

/// A detached file call keeps the Scratch Space membership stable through settlement.
pub(crate) async fn settle_file_context(
	store: &Store,
	access: &mut Access,
	run: &RunMetadata,
) -> Result<Option<PodContext>> {
	context_for_tool(store, access, run, true).await
}

async fn context_for_tool(
	store: &Store,
	access: &mut Access,
	run: &RunMetadata,
	settling_file: bool,
) -> Result<Option<PodContext>> {
	if store.capabilities.0.runner.is_none() {
		return Ok(None);
	}
	let policy = policy(store)?;
	let mut services = scratch::NativeScratchServices { policy: &policy };
	let (space_id, starter) = services.run_binding_metadata(access, run).await?;
	let tenant = access.identity.tenant.clone();
	if run_revoked(&mut **access.tx, &tenant, run.id).await? {
		return Err(NativeError::Forbidden.into());
	}
	let space = if settling_file {
		scratch::hold_for_use(&mut **access.tx, &tenant, space_id).await?
	} else {
		scratch::read(&mut **access.tx, &tenant, space_id).await?
	}
	.ok_or_else(|| NativeError::NotFound("Scratch Space".into()))?;
	if !space.may_use(&starter) || space.node != run.node {
		return Err(NativeError::Forbidden.into());
	}
	Ok(Some(context(store, &policy, &space, &tenant, run.id)?))
}

/// Stop the ended Run's Python session in its Execution Pod: each session
/// holds one of the Space's session slots until it stops, and nothing else
/// stops it. Nothing happens for a Run that ran no Python cell.
pub(crate) async fn stop_python(store: &Store, run: Uuid) -> NativeResult<()> {
	if store.capabilities.0.runner.is_none() {
		return Ok(());
	}
	let space: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.expr(Expr::cust("input->>'scratch_space_id'"))
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
			.and_where(
				Expr::col(Alias::new("kind")).eq(aidash_application::capabilities::pod::PYTHON),
			)
			.and_where(Expr::cust(POD_ROWS))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(store.pool.in_scope(&MAINTENANCE))
	.await?;
	let Some(space) = space else {
		return Ok(());
	};
	let runner = crate::bootstrap::operation_runner(store)?;
	runner
		.python_stop(&space, &run.to_string())
		.await
		.map_err(|error| NativeError::External(error.to_string()))
}

/// The Scratch Space whose Execution Pod a Run's tools use.
pub(crate) async fn run_space(
	store: &Store,
	access: &mut Access,
	run: &RunMetadata,
) -> Result<Uuid> {
	let policy = policy(store)?;
	let mut services = scratch::NativeScratchServices { policy: &policy };
	let (space_id, _) = services.run_binding_metadata(access, run).await?;
	Ok(space_id)
}

/// Stop a Live preview, a Background process of its Run in the Execution Pod.
/// Returns whether the Runner confirmed the stop; a failed stop request is an
/// error so callers can tell it from an unconfirmed stop (#781).
pub(crate) async fn stop_preview(store: &Store, preview: Uuid) -> NativeResult<bool> {
	let runner = crate::bootstrap::operation_runner(store)?;
	aidash_application::capabilities::pod::previews::stop(&runner, preview)
		.await
		.map_err(|error| NativeError::External(error.to_string()))?;
	Ok(true)
}

/// Active Pod operations one sweep looks at.
const SCAN_BATCH: usize = 16;

/// Where the last sweep stopped. A sweep writes only rows that changed, so
/// ordering by `updated_at` let 16 quiet long-running commands take every
/// slot and starve new ones. Sweeps instead walk the active rows in id order
/// from here and wrap around, so each row is seen within a few sweeps.
static SCAN_AFTER: std::sync::Mutex<Option<Uuid>> = std::sync::Mutex::new(None);

/// The next `SCAN_BATCH` active Pod operations after `after` in id order.
async fn scan_after(store: &Store, after: Option<Uuid>) -> NativeResult<Vec<Uuid>> {
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.from(Alias::new("core_operations"))
		.and_where(Expr::col(Alias::new("state")).is_in([
			"prepared",
			"submitted",
			"running",
			"cancelling",
		]))
		.and_where(Expr::cust(POD_ROWS))
		.order_by(Alias::new("id"), Order::Asc)
		.limit(SCAN_BATCH as u64);
	if let Some(after) = after {
		query.and_where(Expr::col(Alias::new("id")).gt(Expr::value(after)));
	}
	crate::database::native::query_scalar(&query.to_string(PostgresQueryBuilder))
		.scalar_all(store.pool.in_scope(&MAINTENANCE))
		.await
}

/// One sweep's batch: the rows after the cursor, wrapping to the start of the
/// id order when fewer remain.
pub(crate) async fn next_batch(store: &Store) -> NativeResult<Vec<Uuid>> {
	let after = *SCAN_AFTER
		.lock()
		.unwrap_or_else(|poisoned| poisoned.into_inner());
	let mut ids = scan_after(store, after).await?;
	if ids.len() < SCAN_BATCH && after.is_some() {
		for id in scan_after(store, None).await? {
			if ids.len() == SCAN_BATCH {
				break;
			}
			if !ids.contains(&id) {
				ids.push(id);
			}
		}
	}
	*SCAN_AFTER
		.lock()
		.unwrap_or_else(|poisoned| poisoned.into_inner()) = if ids.len() == SCAN_BATCH {
		ids.last().copied()
	} else {
		None
	};
	Ok(ids)
}

#[async_trait]
impl PodOperationRepository for Repository<'_> {
	async fn pending(&self) -> Result<Vec<Uuid>> {
		Ok(next_batch(self.store).await?)
	}

	async fn load(&self, id: Uuid) -> Result<PodOperation> {
		let operation: Operation = crate::database::native::query_as(
			&crate::apps::execution::capabilities::services::sessions::select("core_operations")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::cust(POD_ROWS))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(self.store.pool.in_scope(&MAINTENANCE))
		.await?
		.ok_or_else(|| NativeError::NotFound("operation unavailable".into()))?;
		let phase: Option<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("phase"))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(operation.run_id)))
				.and_where(
					Expr::col(Alias::new("tenant")).eq(Expr::value(operation.tenant.clone())),
				)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(self.store.pool.in_scope(&MAINTENANCE))
		.await?;
		Ok(PodOperation {
			id: operation.id,
			tenant: operation.tenant,
			run_id: operation.run_id,
			kind: operation.kind,
			state: operation.state,
			input: operation.input,
			result: operation.result,
			run_finished: phase
				.as_deref()
				.is_none_or(|phase| matches!(phase, "COMPLETED" | "FAILED" | "CANCELLED")),
		})
	}

	async fn context(&self, operation: &PodOperation) -> Result<PodContext> {
		let policy = policy(self.store)?;
		// A Thread never changes its Scratch Space (#697), and a deleted one
		// fails `scratch::read`, so the recorded id stays the Run's space.
		let space_id: Uuid = operation.input["scratch_space_id"]
			.as_str()
			.and_then(|id| id.parse().ok())
			.ok_or(NativeError::Forbidden)?;
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&operation.tenant)?)
			.await?;
		if run_revoked(&mut *tx, &operation.tenant, operation.run_id).await? {
			return Err(NativeError::Forbidden.into());
		}
		let space = scratch::read(&mut *tx, &operation.tenant, space_id).await?;
		tx.commit().await?;
		let space = space.ok_or_else(|| NativeError::NotFound("Scratch Space".into()))?;
		Ok(context(
			self.store,
			&policy,
			&space,
			&operation.tenant,
			operation.run_id,
		)?)
	}

	async fn save(&self, id: Uuid, expected: &str, state: &str, result: &Value) -> Result<bool> {
		let written = crate::database::native::query(
			&Query::update()
				.table(Alias::new("core_operations"))
				.value(Alias::new("state"), state.to_owned())
				.value(Alias::new("result"), result.clone())
				.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
				.and_where(Expr::col(Alias::new("state")).eq(Expr::value(expected.to_owned())))
				.and_where(Expr::cust(POD_ROWS))
				.to_string(PostgresQueryBuilder),
		)
		.execute(self.store.pool.in_scope(&MAINTENANCE))
		.await?
		.rows_affected();
		Ok(written > 0)
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn pod_rows_are_selected_by_their_recorded_runtime() {
		assert_eq!(
			super::POD_ROWS,
			format!(
				"input->>'runtime' = '{}'",
				aidash_domain::capabilities::operations::POD_RUNTIME
			)
		);
	}
}
