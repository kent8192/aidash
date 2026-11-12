//! Complete durable output is paged without losing writer-stop or uncertain-effect evidence.
use crate::{Error, Result, ports::capabilities::projection::OutputReader};
use aidash_domain::capabilities::operations::{
	MountedFile as FileEntry,
	projection::{OperationResult, OperationView},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
/// Raw byte positions may split valid UTF-8 characters, but invalid bytes are
/// independently replaceable and still occupy exactly one output byte.
pub(super) fn boundary(bytes: &[u8], position: usize) -> bool {
	if position == 0 || position == bytes.len() || bytes[position] & 0xc0 != 0x80 {
		return true;
	}
	let mut first = position;
	while first > 0 && bytes[first] & 0xc0 == 0x80 {
		first -= 1;
	}
	let width = match bytes[first] {
		0xc2..=0xdf => 2,
		0xe0..=0xef => 3,
		0xf0..=0xf4 => 4,
		_ => return true,
	};
	let end = first + width;
	end > bytes.len() || position >= end || std::str::from_utf8(&bytes[first..end]).is_err()
}
pub async fn result(
	scope: &mut dyn OutputReader,
	operation: &OperationView<'_>,
	offset: usize,
) -> Result<OperationResult> {
	let (bytes, base) = if let Some(file) = operation.result.get("output_file") {
		let entry: FileEntry = serde_json::from_value(file.clone())?;
		(scope.read(&entry).await?, 0)
	} else {
		let base = usize::try_from(operation.result["preview_offset"].as_u64().unwrap_or(0))
			.map_err(|_| Error::Invalid("INVALID_READ_RANGE".into()))?;
		let bytes = if let Some(encoded) = operation.result["preview_bytes"].as_str() {
			STANDARD
				.decode(encoded)
				.map_err(|_| Error::Invalid("invalid output preview".into()))?
		} else {
			operation.result["preview"]
				.as_str()
				.unwrap_or_default()
				.as_bytes()
				.to_vec()
		};
		(bytes, base)
	};
	let start = offset.max(base) - base;
	if start > bytes.len() || !boundary(&bytes, start) {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let mut end = bytes.len().min(start.saturating_add(scope.read_bytes()?));
	while !boundary(&bytes, end) {
		end -= 1;
	}
	let output = String::from_utf8_lossy(&bytes[start..end]).into_owned();
	let active = matches!(
		operation.state,
		"prepared" | "submitted" | "running" | "cancelling"
	);
	let output_unchanged = output.is_empty() && (active || offset > 0);
	let cursor = base
		.checked_add(end)
		.ok_or_else(|| Error::Invalid("INVALID_READ_RANGE".into()))?;
	let next_offset = (active || end < bytes.len()).then_some(cursor);
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
		output_unchanged,
		next_offset,
		operation_deadline: operation.command_deadline(),
		truncated: operation.result["truncated"] == true,
		effects_may_have_occurred: operation.state != "prepared"
			&& operation.result["effects_may_have_occurred"] != false,
		error: aidash_domain::capabilities::errors::CapabilityError::stored(
			&operation.result["error"],
		),
		output_stale_seconds: operation.result["output_fetched_at"]
			.as_f64()
			.map(|fetched| {
				(chrono::Utc::now().timestamp_millis() as f64 / 1000.0 - fetched).max(0.0) as u64
			}),
		runtime_millis: operation.result["runtime_millis"].as_u64(),
		deadline_expired: operation.result["deadline_expired"].as_bool(),
	})
}
#[cfg(test)]
mod tests;
