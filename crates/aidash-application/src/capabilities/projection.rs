//! Complete durable output is paged without losing writer-stop or uncertain-effect evidence.
use crate::{Error, Result, ports::capabilities::projection::OutputReader};
use aidash_domain::capabilities::operations::{
	MountedFile as FileEntry,
	projection::{OperationResult, OperationView},
};
use serde_json::{Value, json};
pub async fn result(
	scope: &mut dyn OutputReader,
	operation: &OperationView<'_>,
	offset: usize,
) -> Result<OperationResult> {
	let mut output = operation.result["preview"]
		.as_str()
		.unwrap_or_default()
		.to_owned();
	let mut next_offset = None;
	if let Some(file) = operation.result.get("output_file") {
		let entry: FileEntry = serde_json::from_value(file.clone())?;
		let bytes = scope.read(&entry).await?;
		let text = String::from_utf8_lossy(&bytes);
		if offset > text.len() || !text.is_char_boundary(offset) {
			return Err(Error::Invalid("INVALID_READ_RANGE".into()));
		}
		let mut end = text.len().min(offset.saturating_add(scope.read_bytes()?));
		while !text.is_char_boundary(end) {
			end -= 1;
		}
		output = text[offset..end].into();
		next_offset = (end < text.len()).then_some(end);
	}
	Ok(OperationResult {
		operation_id: operation.id,
		kind: operation.kind.to_owned(),
		status: operation.state.to_owned(),
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
		error: aidash_domain::capabilities::errors::CapabilityError::stored(
			&operation.result["error"],
		),
	})
}
#[cfg(test)]
mod tests;
