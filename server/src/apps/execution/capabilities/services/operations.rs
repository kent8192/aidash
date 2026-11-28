//! Durable operation admission and reconciliation. Workers never run host commands.
use super::contracts::*;
use crate::{Result, authorization::access::Access, domain::Run, store::Store};
use serde_json::Value;
use uuid::Uuid;

pub(in crate::apps::execution) async fn request_files(
	access: &mut Access,
	run: Uuid,
) -> Result<Vec<FileEntry>> {
	super::skills::mounted(access, run).await
}

/// Run the complete capability set for callers that own a single lifecycle.
/// Production supervises each loop separately in bootstrap.
pub async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	use aidash_application::ports::capabilities::processing::CapabilityBackgroundJobs as _;
	let repository = crate::bootstrap::operation_processing_repository(&store);
	let jobs = crate::bootstrap::capability_background_jobs(&store);
	tokio::try_join!(
		Box::pin(aidash_runtime::capabilities::execution_loop(
			&repository,
			stopping.clone()
		)),
		Box::pin(aidash_runtime::capabilities::read_recheck_loop(
			&repository,
			stopping.clone()
		)),
		Box::pin(read_recheck_maintenance(&store, stopping.clone())),
		Box::pin(jobs.network(stopping.clone())),
		Box::pin(jobs.references(stopping.clone())),
		Box::pin(jobs.reclamation(stopping)),
	)
	.map_err(crate::Error::from)?;
	Ok(())
}

pub(crate) async fn read_recheck_maintenance(
	store: &Store,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> aidash_application::Result<()> {
	let repository = crate::bootstrap::operation_processing_repository(store);
	let mut passes = aidash_runtime::background::Passes::new(
		"capabilities.read_recheck_maintenance",
		aidash_application::capabilities::read_recheck::PERIOD,
	);
	while !*stopping.borrow() {
		// Each acknowledgement is durable, so an interrupted pass can start
		// again from the database on restart.
		tokio::select! {
			_ = stopping.changed() => break,
			() = passes.run(async {
				let (recheck, wipe) = tokio::join!(
					repository.maintain_read_recheck_targets(),
					crate::apps::workspaces::repositories::scratch::wipe_pending_space(store),
				);
				recheck?;
				wipe?;
				crate::apps::workspaces::repositories::scratch::wipe_pending_thread(store)
					.await
					.map_err(Into::into)
			}) => {}
		}
		passes.pause(&mut stopping).await;
	}
	Ok(())
}

#[cfg(test)]
fn has_input_path_collision(files: &[FileEntry]) -> bool {
	aidash_domain::capabilities::operations::has_input_path_collision(
		&files.iter().cloned().map(Into::into).collect::<Vec<_>>(),
	)
}

/// Admit a command or Python cell for the Execution Pod of the Run's Scratch
/// Space (#745). The capability worker dispatches it.
pub(crate) async fn prepare_pod(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Value,
	kind: &str,
	scratch_space_id: &str,
) -> Result<Value> {
	let (request, expected_session_id) = if kind == aidash_application::capabilities::pod::PYTHON {
		let python: super::python::Python = serde_json::from_value(input)
			.map_err(|error| crate::Error::Invalid(error.to_string()))?;
		(
			aidash_domain::capabilities::operations::ShellRequest {
				idempotency_key: python.idempotency_key,
				command: python.code,
				timeout_seconds: python.timeout_seconds,
				expected_revision: python.expected_revision,
			},
			python.expected_session_id,
		)
	} else {
		(
			serde_json::from_value::<Shell>(input)
				.map_err(|error| crate::Error::Invalid(error.to_string()))?
				.into(),
			None,
		)
	};
	crate::apps::execution::repositories::node_cancellations::require_area_open(
		&mut **access.tx,
		area.id,
	)
	.await?;
	let working_directory =
		crate::apps::execution::repositories::operations::pods::working_directory(
			access,
			&run.metadata(),
		)
		.await?;
	aidash_application::capabilities::pod::accept(
		&mut crate::bootstrap::operation_admission_scope(store, access, run, area),
		&run.metadata(),
		request,
		expected_session_id,
		kind,
		scratch_space_id,
		&working_directory,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn poll(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &Area,
	input: OperationInput,
	kind: &str,
	cancel: bool,
) -> Result<Value> {
	aidash_application::capabilities::poll(
		&mut crate::bootstrap::operation_control_scope(store, access, run, area),
		input.operation_id,
		input.offset,
		kind,
		cancel,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn result(
	store: &Store,
	access: &mut Access,
	operation: &Operation,
	offset: usize,
) -> Result<OperationResult> {
	aidash_application::capabilities::projection::result(
		&mut crate::bootstrap::file_scope(Some(store), access, None),
		&operation.view(),
		offset,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}

pub(crate) async fn verified_health(store: &Store, python: bool) -> Result<Value> {
	let profile = crate::bootstrap::runner_health_profile(store)?;
	aidash_application::capabilities::runner::verified_health(
		&crate::bootstrap::operation_runner(store)?,
		&profile,
		python,
	)
	.await
	.map_err(Into::into)
}

pub(crate) async fn remote(
	store: &Store,
	method: reqwest::Method,
	path: &str,
	body: Option<Value>,
) -> Result<Value> {
	use aidash_application::ports::capabilities::runner::RunnerTransport as _;
	crate::bootstrap::operation_runner(store)?
		.request(method.as_str(), path, body.as_ref())
		.await
		.map_err(Into::into)
}

#[cfg(test)]
#[path = "../tests/services_operations_tests.rs"]
mod tests;

#[cfg(test)]
use aidash_domain::capabilities::operations::runner::rounded_working_bytes;

pub(crate) use crate::apps::execution::repositories::operations::{
	Operation, get, persist, set_area,
};
