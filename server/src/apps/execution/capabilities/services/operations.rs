//! Durable operation admission and reconciliation. Workers never run host commands.
use super::{contracts::*, service};
use crate::{Error, Result, authorization::access::Access, domain::Run, store::Store};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) async fn prepare(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Shell,
	_key: &str,
) -> Result<Value> {
	prepare_kind(store, access, run, area, input, "shell", json!({})).await
}

pub(in crate::apps::execution) async fn request_files(
	access: &mut Access,
	run: Uuid,
	area: &Area,
	package_files: Vec<FileEntry>,
) -> Result<Vec<FileEntry>> {
	let mut files = service::files(area)?;
	files.extend(super::skills::mounted(access, run).await?);
	files.extend(package_files);
	Ok(files)
}

#[cfg(test)]
fn has_input_path_collision(files: &[FileEntry]) -> bool {
	aidash_domain::capabilities::operations::has_input_path_collision(
		&files.iter().cloned().map(Into::into).collect::<Vec<_>>(),
	)
}

pub(crate) async fn prepare_kind(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Shell,
	kind: &str,
	extra: Value,
) -> Result<Value> {
	aidash_application::capabilities::prepare(
		&mut crate::bootstrap::operation_admission_scope(store, access, run, area),
		&run.metadata(),
		input.into(),
		kind,
		extra,
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
	let mut output = operation.result["preview"]
		.as_str()
		.unwrap_or_default()
		.to_owned();
	let mut next_offset = None;
	if let Some(file) = operation.result.get("output_file") {
		let entry: FileEntry = serde_json::from_value(file.clone())?;
		let bytes = store.capabilities.read(access, &entry).await?;
		let text = String::from_utf8_lossy(&bytes);
		if offset > text.len() || !text.is_char_boundary(offset) {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		let mut end = text
			.len()
			.min(offset.saturating_add(store.capabilities.0.limits.read_bytes));
		while !text.is_char_boundary(end) {
			end -= 1;
		}
		output = text[offset..end].into();
		next_offset = (end < text.len()).then_some(end);
	}
	Ok(OperationResult {
		operation_id: operation.id,
		kind: operation.kind.clone(),
		status: operation.state.clone(),
		area_id: operation.area_id,
		generation: operation.generation,
		revision: operation.revision,
		epoch: operation.epoch,
		policy_revision: operation.policy_revision,
		termination_confirmed: operation.result["termination_confirmed"] == true,
		writer_frozen: operation.result["writer_frozen"] == true,
		session_id: serde_json::from_value(
			operation
				.input
				.get("session_id")
				.cloned()
				.unwrap_or(Value::Null),
		)?,
		displays: serde_json::from_value(
			operation
				.result
				.get("displays")
				.cloned()
				.unwrap_or(json!([])),
		)?,
		exit_code: operation.result["exit_code"].as_i64(),
		output,
		next_offset,
		truncated: operation.result["truncated"] == true,
		effects_may_have_occurred: operation.state != "prepared"
			&& operation.result["effects_may_have_occurred"] != false,
		error: super::errors::CapabilityError::stored(&operation.result["error"]),
	})
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

pub async fn run(store: Store, stopping: tokio::sync::watch::Receiver<bool>) -> Result<()> {
	aidash_runtime::capabilities::run(
		&crate::bootstrap::operation_processing_repository(&store),
		&crate::bootstrap::capability_background_jobs(&store),
		stopping,
	)
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
