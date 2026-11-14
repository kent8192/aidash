use super::RunnerHttp;
use aidash_application::ports::capabilities::execution_pod::*;
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

impl RunnerHttp {
	async fn pod_call<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
		let body = serde_json::to_value(body).map_err(aidash_application::Error::from)?;
		let (status, value) = self.raw_request("POST", path, Some(&body)).await?;
		if !status.is_success() {
			let code = value
				.get("code")
				.and_then(Value::as_str)
				.unwrap_or("invalid_error_response");
			let message = value
				.get("message")
				.and_then(Value::as_str)
				.unwrap_or("Runner returned an invalid error response");
			return Err(ExecutionPodError::Remote {
				code: code.into(),
				message: message.into(),
			});
		}
		serde_json::from_value(value)
			.map_err(aidash_application::Error::from)
			.map_err(Into::into)
	}
	async fn pod_effect(&self, path: &str, body: &impl Serialize, field: &str) -> Result<()> {
		let value: Value = self.pod_call(path, body).await?;
		if value.get(field) != Some(&Value::Bool(true)) {
			return Err(ExecutionPodError::Transport(
				aidash_application::Error::External(format!("Runner did not confirm {field}")),
			));
		}
		Ok(())
	}
}
#[async_trait]
impl ExecutionPodRuntime for RunnerHttp {
	async fn ensure(&self, settings: &ExecutionPodSettings) -> Result<PodReceipt> {
		self.pod_call("/v2/pods/ensure", settings).await
	}
	async fn execute(&self, request: &CommandRequest) -> Result<CommandResult> {
		self.pod_call("/v2/commands/submit", request).await
	}
	async fn result(&self, operation_id: &str) -> Result<CommandResult> {
		self.pod_call("/v2/commands/result", &json!({"operation_id":operation_id}))
			.await
	}
	async fn cancel_waiting(&self, scratch_space_id: &str, run_id: &str) -> Result<()> {
		self.pod_effect(
			"/v2/runs/cancel",
			&json!({"scratch_space_id":scratch_space_id,"run_id":run_id}),
			"cancelled_waiting",
		)
		.await
	}
	async fn backgrounds(&self, scratch_space_id: &str) -> Result<BackgroundProcesses> {
		self.pod_call(
			"/v2/background/list",
			&json!({"scratch_space_id":scratch_space_id}),
		)
		.await
	}
	async fn stop_background(&self, operation_id: &str) -> Result<CommandResult> {
		self.pod_call("/v2/background/stop", &json!({"operation_id":operation_id}))
			.await
	}
	async fn python_start(&self, request: &PythonStartRequest) -> Result<PythonSession> {
		self.pod_call("/v2/python/start", request).await
	}
	async fn stop_run(&self, scratch_space_id: &str, run_id: &str) -> Result<Vec<String>> {
		#[derive(serde::Deserialize)]
		struct Stopped {
			stopped: Vec<String>,
		}
		let result: Stopped = self
			.pod_call(
				"/v2/runs/stop",
				&json!({"scratch_space_id":scratch_space_id,"run_id":run_id}),
			)
			.await?;
		Ok(result.stopped)
	}
	async fn background_log(&self, request: &LogRequest) -> Result<LogChunk> {
		self.pod_call("/v2/background/log", request).await
	}
	async fn file_read(&self, request: &FileReadRequest) -> Result<FileChunk> {
		self.pod_call("/v2/files/read", request).await
	}
	async fn file_search(&self, request: &FileSearchRequest) -> Result<FileSearchResult> {
		self.pod_call("/v2/files/search", request).await
	}
	async fn file_patch(&self, request: &FilePatchRequest) -> Result<FilePatchResult> {
		self.pod_call("/v2/files/patch", request).await
	}
	async fn file_write(&self, request: &FileWriteRequest) -> Result<FileWriteResult> {
		self.pod_call("/v2/files/write", request).await
	}
	async fn file_manifest(&self, request: &FileManifestRequest) -> Result<FileManifestResult> {
		self.pod_call("/v2/files/manifest", request).await
	}
	async fn preview_start(&self, request: &PreviewStartRequest) -> Result<PreviewTarget> {
		self.pod_call("/v2/previews/start", request).await
	}
	async fn mcp_start(&self, request: &McpStartRequest) -> Result<McpSession> {
		self.pod_call("/v2/mcp/start", request).await
	}
	async fn mcp_call(&self, request: &McpCallRequest) -> Result<McpCallResult> {
		self.pod_call("/v2/mcp/call", request).await
	}
	async fn preview_stop(&self, operation_id: &str) -> Result<()> {
		let _: Value = self
			.pod_call("/v2/previews/stop", &json!({"operation_id":operation_id}))
			.await?;
		Ok(())
	}
	async fn mcp_stop(&self, operation_id: &str) -> Result<()> {
		let _: Value = self
			.pod_call("/v2/mcp/stop", &json!({"operation_id":operation_id}))
			.await?;
		Ok(())
	}
	async fn python_cell(&self, request: &CommandRequest) -> Result<CommandResult> {
		self.pod_call("/v2/python/cell", request).await
	}
	async fn python_stop(&self, scratch_space_id: &str, run_id: &str) -> Result<()> {
		let _: Value = self
			.pod_call(
				"/v2/python/stop",
				&json!({"scratch_space_id":scratch_space_id,"run_id":run_id}),
			)
			.await?;
		Ok(())
	}
	async fn change_log(
		&self,
		scratch_space_id: &str,
		offset: u64,
		limit: u32,
	) -> Result<ChangeLogChunk> {
		self.pod_call(
			"/v2/changes",
			&json!({"scratch_space_id":scratch_space_id,"offset":offset,"limit":limit}),
		)
		.await
	}
	async fn viewer_heartbeat(&self, scratch_space_id: &str, viewer_id: &str) -> Result<()> {
		self.pod_effect(
			"/v2/viewers/heartbeat",
			&json!({"scratch_space_id":scratch_space_id,"viewer_id":viewer_id}),
			"active",
		)
		.await
	}
	async fn reset(&self, settings: &ExecutionPodSettings) -> Result<PodReceipt> {
		self.pod_call("/v2/pods/reset", settings).await
	}
	async fn stop(&self, scratch_space_id: &str) -> Result<()> {
		self.pod_effect(
			"/v2/pods/stop",
			&json!({"scratch_space_id":scratch_space_id}),
			"stopped",
		)
		.await
	}
	async fn delete(&self, scratch_space_id: &str) -> Result<()> {
		self.pod_effect(
			"/v2/pods/delete",
			&json!({"scratch_space_id":scratch_space_id}),
			"deleted",
		)
		.await
	}
}
