//! Background processes of a Run in its Execution Pod: what a foreground
//! command left running when it ended (F32), and Live previews (#745). A Run
//! sees and stops only its own.
use super::files::in_pod;
use crate::{
	Error, Result,
	ports::capabilities::{
		execution_pod::{CommandResult, LogRequest},
		pod_operations::PodContext,
	},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};

/// The most log bytes one read returns, as the Runner allows.
pub const LOG_BYTES: u32 = 262_144;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessLog {
	pub operation_id: String,
	/// `stdout` unless given.
	pub stream: Option<String>,
	pub offset: Option<u64>,
	pub limit: Option<u32>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessStop {
	pub operation_id: String,
}

fn view(process: &CommandResult) -> Value {
	json!({
		"operation_id": process.operation_id,
		"status": process.status,
		"exit_code": process.exit_code,
		"stop_reason": process.stop_reason,
	})
}

async fn owned(context: &PodContext, run_id: &str) -> Result<Vec<CommandResult>> {
	let listed = in_pod!(
		context,
		context.runtime.backgrounds(context.scratch_space_id())
	)?;
	Ok(listed
		.processes
		.into_iter()
		.filter(|process| process.run_id.as_deref() == Some(run_id))
		.collect())
}

/// This Run's Background processes in the Scratch Space's Pod.
pub async fn list(context: &PodContext, run_id: &str) -> Result<Value> {
	let processes = owned(context, run_id).await?;
	Ok(json!({"processes": processes.iter().map(view).collect::<Vec<_>>()}))
}

async fn require_owned(context: &PodContext, run_id: &str, operation_id: &str) -> Result<()> {
	if owned(context, run_id)
		.await?
		.iter()
		.any(|process| process.operation_id == operation_id)
	{
		Ok(())
	} else {
		Err(Error::NotFound(format!(
			"Background process unavailable: {operation_id}"
		)))
	}
}

/// Read a bounded part of a Background process's output. Without `offset` it
/// returns the last `limit` bytes.
pub async fn log(context: &PodContext, run_id: &str, input: ProcessLog) -> Result<Value> {
	let stream = input.stream.unwrap_or_else(|| "stdout".into());
	if !matches!(stream.as_str(), "stdout" | "stderr") {
		return Err(Error::Invalid("stream must be stdout or stderr".into()));
	}
	let limit = input.limit.unwrap_or(65_536);
	if limit == 0 || limit > LOG_BYTES {
		return Err(Error::Invalid(format!(
			"limit must be between 1 and {LOG_BYTES}"
		)));
	}
	require_owned(context, run_id, &input.operation_id).await?;
	let request = LogRequest {
		operation_id: input.operation_id.clone(),
		stream,
		offset: input.offset,
		limit,
	};
	let chunk = in_pod!(context, context.runtime.background_log(&request))?;
	let bytes = STANDARD
		.decode(&chunk.data)
		.map_err(|_| Error::External("invalid Runner log encoding".into()))?;
	Ok(json!({
		"operation_id": input.operation_id,
		"content": String::from_utf8_lossy(&bytes),
		"next_offset": chunk.next_offset,
		"eof": chunk.eof,
		"size": chunk.size,
	}))
}

/// Stop one of this Run's Background processes.
pub async fn stop(context: &PodContext, run_id: &str, input: ProcessStop) -> Result<Value> {
	require_owned(context, run_id, &input.operation_id).await?;
	let stopped = in_pod!(
		context,
		context.runtime.stop_background(&input.operation_id)
	)?;
	Ok(view(&stopped))
}

#[cfg(test)]
mod tests;
