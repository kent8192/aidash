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

/// The operator policy Pod settings and Run credentials come from. A process
/// that never built a Federation has none, and keeps tools on the Runner's
/// per-command path.
fn policy(store: &Store) -> Option<ScratchOperatorPolicy> {
	store
		.in_process
		.get()
		.map(|bound| bound.config.scratch_policy.clone())
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
/// is now. None when this Deployment runs no Execution Pods.
pub(crate) async fn tool_context(
	store: &Store,
	access: &mut Access,
	run: &RunMetadata,
) -> Result<Option<PodContext>> {
	if store.capabilities.0.runner.is_none() {
		return Ok(None);
	}
	let Some(policy) = policy(store) else {
		return Ok(None);
	};
	let mut services = scratch::NativeScratchServices { policy: &policy };
	let (space_id, _) = services.run_binding_metadata(access, run).await?;
	let tenant = access.identity.tenant.clone();
	let space = scratch::read(&mut **access.tx, &tenant, space_id)
		.await?
		.ok_or_else(|| NativeError::NotFound("Scratch Space".into()))?;
	Ok(Some(context(store, &policy, &space, &tenant, run.id)?))
}

/// Stop a Live preview that runs as a Background process in an Execution Pod
/// (#745). None where previews still run on the Runner's own Pods.
pub(crate) async fn stop_preview(store: &Store, preview: Uuid) -> Option<NativeResult<bool>> {
	if store.capabilities.0.runner.is_none() || policy(store).is_none() {
		return None;
	}
	let runner = match crate::bootstrap::operation_runner(store) {
		Ok(runner) => runner,
		Err(error) => return Some(Err(error)),
	};
	Some(Ok(aidash_application::capabilities::pod::previews::stop(
		&runner, preview,
	)
	.await
	.is_ok()))
}

#[async_trait]
impl PodOperationRepository for Repository<'_> {
	async fn pending(&self) -> Result<Vec<Uuid>> {
		let ids: Vec<Uuid> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("core_operations"))
				.and_where(Expr::col(Alias::new("state")).is_in([
					"prepared",
					"submitted",
					"running",
					"cancelling",
				]))
				.and_where(Expr::cust(POD_ROWS))
				.order_by(Alias::new("updated_at"), Order::Asc)
				.limit(16)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(self.store.pool.in_scope(&MAINTENANCE))
		.await?;
		Ok(ids)
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
				.from(Alias::new("runs"))
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
		let policy = policy(self.store).ok_or_else(|| {
			NativeError::Conflict("RUNTIME_UNAVAILABLE: no Scratch Space policy".into())
		})?;
		// A Workspace never changes its Scratch Space (#697), and a deleted one
		// fails `scratch::read`, so the recorded id stays the Run's space.
		let space_id: Uuid = operation.input["scratch_space_id"]
			.as_str()
			.and_then(|id| id.parse().ok())
			.ok_or(NativeError::Forbidden)?;
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&operation.tenant)?)
			.await
			.map_err(NativeError::from)?;
		let space = scratch::read(&mut *tx, &operation.tenant, space_id).await?;
		tx.commit().await.map_err(NativeError::from)?;
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
