//! Capability operations retain current authority, idempotency and writer admission order.
use crate::{
	Error, Result,
	ports::capabilities::{AcceptedOperation, OperationAdmissionScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::operations::{
		ShellRequest, available, package_files, request_validation_failure,
	},
};
use serde_json::{Value, json};
pub async fn prepare<O: Send>(
	scope: &mut dyn OperationAdmissionScope<O>,
	run: &RunMetadata,
	input: ShellRequest,
	kind: &str,
	extra: Value,
) -> Result<Value> {
	let limits = scope.limits();
	let seconds = input.seconds(limits)?;
	let digest = input.digest(kind, run.id, &extra);
	let key = input.key(run.id);
	if let Some((previous, previous_digest)) = scope.previous(&key).await? {
		if previous_digest != digest {
			return Err(Error::Conflict("IDEMPOTENCY_CONFLICT".into()));
		}
		return scope.result(&previous, 0).await;
	}
	let area = scope.area();
	available(&area.state)?;
	if !limits.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	if input.expected_revision != area.revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if scope.active_run().await? != Some(run.id) || run.phase.is_terminal() {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	scope.require_write().await?;
	let files = scope.request_files(run.id, package_files(&extra)?).await?;
	if let Some((code, _)) = request_validation_failure(limits.working_bytes, &files) {
		return Err(Error::Conflict(code.into()));
	}
	scope.verified_health(kind == "code_interpreter").await?;
	if matches!(kind, "shell" | "python_install") {
		scope
			.release_python(if kind == "python_install" {
				"packages_changed"
			} else {
				"shell_requested"
			})
			.await?;
	}
	let id = scope.operation_id();
	let epoch = area.epoch + 1;
	scope.set_running(epoch).await?;
	let mut value = json!({"command":input.command,"seconds":seconds});
	value
		.as_object_mut()
		.unwrap()
		.extend(extra.as_object().ok_or(Error::Forbidden)?.clone());
	let operation = scope
		.accept(AcceptedOperation {
			id,
			key,
			digest,
			kind: kind.into(),
			epoch,
			input: value,
		})
		.await?;
	scope.result(&operation, 0).await
}
#[cfg(test)]
mod tests;

/// Control never accepts a foreign principal, Area, Run or incompatible operation kind.
pub async fn poll(
	scope: &mut dyn crate::ports::capabilities::OperationControlScope,
	id: uuid::Uuid,
	offset: Option<usize>,
	kind: &str,
	cancel: bool,
) -> Result<Value> {
	let operation = scope.load(id).await?;
	if !operation.visible_to(scope.area_id(), scope.run_id(), scope.principal(), kind) {
		return Err(Error::NotFound("operation unavailable".into()));
	}
	if cancel && let Some(change) = operation.cancellation() {
		let never_dispatched = change.never_dispatched;
		scope.apply_cancellation(change)?;
		if never_dispatched {
			scope.activate_area().await?;
			if operation.kind == "code_interpreter" {
				scope.complete_python_without_writer().await?;
			}
		}
		scope.persist().await?;
	}
	scope.project(offset.unwrap_or(0)).await
}

pub mod withdrawal;

pub mod runner;

pub mod reconciliation;

pub mod processing;
