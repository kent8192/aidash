//! Owned native worker boundaries preserve authority, visibility and lease fencing.
use super::{RecoveryRepository, store::run_state::FailureDelivery};
use crate::{
	Error as NativeError,
	authorization::execution::{self, DeliveryGuard, Guard},
	federation::{Federation, Home},
	store::Store,
	transactions::gate::ReadLease,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::{
	Error, Result,
	ports::{
		ExecutionRecoveryStore,
		execution::{
			terminal::{FailureScope, TerminalRepository},
			worker::{WorkerLeases, WorkerStep},
		},
	},
	recovery::{AuthorityFailure, ExecutionFailure},
};
use aidash_domain::{Run, RunControl, RunMetadata, Task, TaskStatus};
use async_trait::async_trait;
use uuid::Uuid;

/// The worker's lease bookkeeping for a Run it claimed, before or outside the
/// step's authority, reaches the Run whichever Tenant owns it.
pub(crate) fn claimed() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::ActivationClaim)
}

pub(crate) struct Leases {
	pub(crate) store: Store,
}
#[async_trait]
impl WorkerLeases for Leases {
	async fn current_id(&self, token: Uuid) -> Result<Uuid> {
		self.store
			.pool
			.atomic(&claimed(), async |tx| {
				super::super::models::Run::leased_id(tx, token).await
			})
			.await?
			.ok_or_else(|| Error::Conflict("worker lease lost".into()))
	}
	async fn renew(&self, run: Uuid, token: Uuid, seconds: i32) -> Result<bool> {
		self.store
			.renew_lease(&claimed(), run, token, seconds)
			.await
			.map_err(Into::into)
	}
	fn transient(&self, error: &Error) -> bool {
		matches!(error, Error::TransactionPending)
			|| matches!(error, Error::Port(error)
			if error.downcast_ref::<NativeError>().is_some_and(NativeError::is_transient_database))
	}
	async fn control(&self, run: Uuid) -> Result<RunControl> {
		self.store
			.pool
			.atomic(&claimed(), async |tx| {
				super::super::models::Run::committed_control(tx, run).await
			})
			.await
			.map_err(Into::into)
	}
}

pub(crate) struct Step {
	// Authority rolls back before ordinary visibility and the active-step guard drop.
	pub(crate) guard: Option<Guard>,
	pub(crate) run: Run,
	pub(crate) federation: Federation,
	pub(crate) recovery: RecoveryRepository,
	pub(crate) _active: crate::http::ActiveExecution,
	pub(crate) visibility: ReadLease,
}
#[async_trait]
impl WorkerStep for Step {
	fn metadata(&self) -> RunMetadata {
		self.run.metadata()
	}
	fn lease_recoveries(&self) -> u32 {
		self.run.recovery.lease_recoveries
	}
	async fn tenant(&mut self) -> Option<String> {
		self.federation
			.store
			.run_tenant(self.run.id)
			.await
			.ok()
			.map(|tenant| tenant.as_str().to_owned())
	}
	fn recovery_store(&self) -> &dyn ExecutionRecoveryStore {
		&self.recovery
	}
	fn classify_failure(&self, error: Error) -> ExecutionFailure {
		classify_failure(&NativeError::from(error))
	}
	async fn cancel_scoped(&mut self, token: Uuid) -> Result<bool> {
		execution::cancel_if_scoped(&self.federation.store, &self.run, token)
			.await
			.map_err(Into::into)
	}
	async fn admit(&mut self) -> Result<()> {
		self.guard = Some(Guard::begin(&self.federation, &self.run).await?);
		Ok(())
	}
	async fn invoke(&mut self, token: Uuid) -> Result<()> {
		let environment = crate::bootstrap::execution_environment(
			&self.federation,
			self.guard.as_ref(),
			&self.run,
		);
		let mut visibility = super::agent::Visibility {
			visibility: &mut self.visibility,
			store: &self.federation.store,
		};
		aidash_harness::agent::Executor::new(&environment)
			.advance(&mut self.run, token, &mut visibility)
			.await
	}
	async fn finish_authority(&mut self, result: Result<()>) -> Result<()> {
		if let Some(guard) = self.guard.take() {
			guard
				.finish(result.map_err(NativeError::from))
				.await
				.map_err(Into::into)
		} else {
			result
		}
	}
	async fn resume_visibility(&mut self) -> Result<()> {
		self.visibility
			.resume(&self.federation.store)
			.await
			.map_err(Into::into)
	}
}

pub(crate) struct Terminal {
	pub(crate) federation: Federation,
}
struct Failure {
	guard: Option<DeliveryGuard>,
	record: FailureDelivery,
	federation: Federation,
	token: Uuid,
	_visibility: ReadLease,
}
#[async_trait]
impl TerminalRepository for Terminal {
	async fn claim_failure(
		&self,
		token: Uuid,
		seconds: i32,
	) -> Result<Option<Box<dyn FailureScope>>> {
		let visibility = ReadLease::begin(&self.federation.store).await?;
		let Some(record) = self
			.federation
			.store
			.claim_failure_delivery(token, seconds)
			.await?
		else {
			return Ok(None);
		};
		Ok(Some(Box::new(Failure {
			guard: None,
			record,
			federation: self.federation.clone(),
			token,
			_visibility: visibility,
		})))
	}
	async fn pending_inputs(&self) -> Result<Option<RunMetadata>> {
		self.federation
			.store
			.pending_terminal_run_message()
			.await
			.map_err(Into::into)
	}
	async fn deliver_inputs(&self, run: &RunMetadata) -> Result<()> {
		self.federation
			.deliver_run_message_metadata(run)
			.await
			.map_err(Into::into)
	}
	async fn defer_inputs(&self, run: Uuid) -> Result<()> {
		self.federation
			.store
			.defer_run_message_delivery(&claimed(), run)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl FailureScope for Failure {
	fn metadata(&self) -> &RunMetadata {
		&self.record.metadata
	}
	fn target(&self) -> TaskStatus {
		self.record.target.task_status()
	}
	async fn remote_grant(&mut self) -> Result<bool> {
		Ok(crate::authorization::peer::admission::run_grant(
			&self.federation.store,
			&self.record.metadata,
		)
		.await?
		.is_some())
	}
	async fn admit(&mut self) -> Result<()> {
		self.guard = Some(DeliveryGuard::begin(&self.federation, &self.record.metadata).await?);
		Ok(())
	}
	async fn deliver_inputs(&mut self) -> Result<()> {
		self.federation
			.deliver_run_message_metadata(&self.record.metadata)
			.await
			.map_err(Into::into)
	}
	async fn task(&mut self) -> Result<Task> {
		Home::for_delivery(self.federation.clone(), self.record.metadata.clone())
			.with_authority(self.guard.as_ref().and_then(DeliveryGuard::local_authority))
			.task()
			.await
			.map_err(Into::into)
	}
	async fn transition(&mut self, status: TaskStatus) -> Result<Task> {
		self.federation
			.transition_terminal_metadata(
				&self.record.metadata,
				status,
				self.guard.as_ref().and_then(DeliveryGuard::local_authority),
			)
			.await
			.map_err(Into::into)
	}
	async fn finish_authority(&mut self, result: Result<TaskStatus>) -> Result<TaskStatus> {
		if let Some(guard) = self.guard.take() {
			guard
				.finish(result.map_err(NativeError::from))
				.await
				.map_err(Into::into)
		} else {
			result
		}
	}
	async fn pause_authority(&mut self, failure: &AuthorityFailure) -> Result<()> {
		self.federation
			.store
			.pause_for_execution(
				&self.record.metadata,
				self.token,
				&failure.reason(),
				"run.authorization_blocked",
				failure.refusal(),
			)
			.await
			.map_err(Into::into)
	}
	async fn finish(&mut self, result: Result<TaskStatus>) -> Result<()> {
		self.federation
			.store
			.finish_failure_delivery(
				&claimed(),
				&self.record,
				self.token,
				result.map_err(NativeError::from),
			)
			.await
			.map_err(Into::into)
	}
}

/// Keep backend codes and native failure identity out of the application use case.
pub(crate) fn classify_failure(error: &NativeError) -> ExecutionFailure {
	match error {
		// A local embedding or vector outage is typed `Unavailable` where it
		// occurs, so it retries and pauses like a Home one. `SemanticUnavailable`
		// also reports invariant and allowance violations, which never retry.
		NativeError::RemoteSemantic(reason) => ExecutionFailure::Semantic(*reason),
		NativeError::Context(reason) => ExecutionFailure::Context(*reason),
		NativeError::DeciderOutage => ExecutionFailure::DeciderOutage,
		// The harness owns bounded compact-and-retry; an overflow that escapes
		// it has exhausted recovery and must pause rather than transport-retry.
		NativeError::ContextOverflow => ExecutionFailure::Context(
			aidash_domain::context::recovery::Failure::OverflowRetriesExhausted,
		),
		NativeError::Forbidden | NativeError::Unauthorized => {
			ExecutionFailure::Authority(AuthorityFailure::Denied(None))
		}
		NativeError::AuthorityRefused(refusal) => {
			ExecutionFailure::Authority(AuthorityFailure::Denied(Some((**refusal).clone())))
		}
		NativeError::IdentityStatusUnavailable => {
			ExecutionFailure::Authority(AuthorityFailure::IdentityUnavailable)
		}
		NativeError::MediaRouteUnavailable(_) => ExecutionFailure::MediaRoute(error.to_string()),
		NativeError::TransactionPending | NativeError::StaleInference => ExecutionFailure::Deferred,
		NativeError::ToolRetry {
			message,
			count,
			retry_after,
		} => ExecutionFailure::ToolRetry {
			message: message.clone(),
			count: *count,
			retry_after: *retry_after,
		},
		NativeError::External(_) | NativeError::InferenceStalled => ExecutionFailure::Inference {
			transport: true,
			status: None,
			message: error.to_string(),
		},
		NativeError::ProviderRejected { status, .. } => ExecutionFailure::Inference {
			transport: false,
			status: Some(*status),
			message: error.to_string(),
		},
		// The owner must act (connect, reconnect or wait for their plan's
		// limit); retrying the same Run would not help (ADR 0007).
		NativeError::Plan(_) => ExecutionFailure::Inference {
			transport: false,
			status: None,
			message: error.to_string(),
		},
		_ => ExecutionFailure::Other(error.to_string()),
	}
}
