//! Live previews as Background processes of their Run in the Execution Pod
//! (#745). The Runner answers once the port accepts connections, and the
//! Preview gateway forwards to the address it returns; no files are streamed.
use super::{ensure, tool_error};
use crate::{
	Error, Result,
	ports::capabilities::{
		execution_pod::{
			ExecutionPodError, ExecutionPodErrorCode, ExecutionPodRuntime, PreviewStartRequest,
		},
		pod_operations::PodContext,
	},
};
use uuid::Uuid;

/// Where the gateway connects: the host of the Runner's `<pod-ip>:<port>`
/// answer. The port must be the one the preview was started on.
pub fn host(address: &str, port: u16) -> Result<String> {
	let (host, answered) = address
		.rsplit_once(':')
		.ok_or_else(|| Error::External("the Runner returned no preview address".into()))?;
	let host = host.trim_start_matches('[').trim_end_matches(']');
	if host.is_empty() || answered.parse::<u16>().ok() != Some(port) {
		return Err(Error::External(
			"the Runner returned a preview address on another port".into(),
		));
	}
	Ok(host.to_owned())
}

/// Start the preview command as a Background process of the Run and wait for
/// its port. Returns the host the gateway forwards to. Never call this inside
/// an authority transaction: it waits for the Pod and the port.
pub async fn start(
	context: &PodContext,
	preview: Uuid,
	run: Uuid,
	command: &str,
	port: u16,
	working_directory: String,
) -> Result<String> {
	ensure(context).await.map_err(tool_error)?;
	let target = context
		.runtime
		.preview_start(&PreviewStartRequest {
			operation_id: preview.to_string(),
			scratch_space_id: context.scratch_space_id().to_owned(),
			run_id: run.to_string(),
			code: command.to_owned(),
			cwd: Some(working_directory),
			port,
			proxy_credential: context.credential.clone(),
		})
		.await
		.map_err(tool_error)?;
	host(&target.address, port)
}

/// Stop a preview's process. One the Runner no longer knows is stopped.
pub async fn stop(runtime: &dyn ExecutionPodRuntime, preview: Uuid) -> Result<()> {
	match runtime.preview_stop(&preview.to_string()).await {
		Ok(())
		| Err(ExecutionPodError::Remote {
			code: ExecutionPodErrorCode::CommandNotFound,
			..
		}) => Ok(()),
		Err(error) => Err(tool_error(error)),
	}
}

#[cfg(test)]
mod tests;
