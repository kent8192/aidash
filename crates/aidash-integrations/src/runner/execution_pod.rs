use super::{MAX_RUNNER_RESPONSE_BYTES, RunnerHttp};
use aidash_application::ports::capabilities::execution_pod::*;
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::time::Duration;

/// How long a stop waits for the Runner to show the process ended.
const STOP_CONFIRMATION: Duration = Duration::from_secs(15);
const STOP_POLL: Duration = Duration::from_millis(300);

fn pod_timeout(path: &str, body: &Value) -> Duration {
	let seconds = match path {
		// The preview start waits 60 s (runner/execution_pod.py:2637).
		"/v2/previews/start" => 75,
		// The stdio process starts within a 300 s window (runner/execution_pod.py:2497).
		"/v2/mcp/start" => 315,
		// The Runner gives the exchange timeout + 10 s (runner/execution_pod.py:2534).
		"/v2/mcp/call" => body
			.get("timeout_seconds")
			.and_then(Value::as_u64)
			.unwrap_or(0)
			.saturating_add(15),
		// These routes share the 60 s stop loop (runner/execution_pod.py:2286).
		"/v2/runs/stop" | "/v2/mcp/stop_run" | "/v2/runs/stop_sessions" | "/v2/python/stop" => 75,
		// Python start/cell cover Kubernetes lookup via kube() (runner/control.py:420).
		"/v2/python/start" | "/v2/python/cell" => 60,
		// Pod creation includes the 1800 s readiness wait (runner/execution_pod.py:1016, ADR 0113).
		path if path.starts_with("/v2/pods/") => 1900,
		// The default covers the 45 s kube() deadline (runner/control.py:420).
		_ => 60,
	};
	Duration::from_secs(seconds)
}

impl RunnerHttp {
	/// Ask for proof that an adopted Background process can no longer run.
	pub async fn background_ended(
		&self,
		scratch_space_id: &str,
		run_id: &str,
		operation_id: &str,
		pod_uid: Option<&str>,
	) -> Result<bool> {
		#[derive(serde::Deserialize)]
		struct Evidence {
			ended: bool,
		}
		let evidence: Evidence = self
			.pod_call(
				"/v2/background/ended",
				&json!({
					"scratch_space_id": scratch_space_id, "run_id": run_id,
					"operation_id": operation_id, "pod_uid": pod_uid,
				}),
			)
			.await?;
		Ok(evidence.ended)
	}

	/// Fence future starts and wait for this Run's Python and stdio sessions to stop.
	pub async fn stop_sessions(&self, scratch_space_id: &str, run_id: &str) -> Result<()> {
		#[derive(serde::Deserialize)]
		struct Confirmation {
			#[serde(rename = "stopped")]
			_stopped: Vec<String>,
		}
		let _: Confirmation = self
			.pod_call(
				"/v2/runs/stop_sessions",
				&json!({
					"scratch_space_id": scratch_space_id, "run_id": run_id,
				}),
			)
			.await?;
		Ok(())
	}

	async fn pod_call<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
		let body = serde_json::to_value(body).map_err(aidash_application::Error::from)?;
		let url = reqwest::Url::parse(&self.endpoint)
			.map_err(|_| aidash_application::Error::Invalid("invalid runner endpoint".into()))?;
		if url.scheme() != "https"
			&& !(url.scheme() == "http"
				&& url.host_str().is_some_and(|host| {
					matches!(host, "127.0.0.1" | "[::1]") || host.ends_with(".svc.cluster.local")
				})) {
			return Err(aidash_application::Error::Invalid(
				"runner transport requires TLS, loopback or a cluster Service".into(),
			)
			.into());
		}
		let token = self
			.credentials
			.resolve(&self.credential_env)
			.map_err(|_| {
				aidash_application::Error::Conflict(
					"RUNTIME_UNAVAILABLE: runner credential unavailable".into(),
				)
			})?;
		let client = reqwest::Client::builder()
			.no_proxy()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.map_err(crate::http_error)?;
		let deadline = pod_timeout(path, &body);
		let (status, value): (_, Value) = tokio::time::timeout(deadline, async {
			let response = client
				.post(format!("{}{path}", self.endpoint.trim_end_matches('/')))
				.bearer_auth(token)
				.json(&body)
				.send()
				.await
				.map_err(crate::http_error)?;
			let status = response.status();
			let value = crate::response::json(response, MAX_RUNNER_RESPONSE_BYTES).await?;
			Ok::<_, aidash_application::Error>((status, value))
		})
		.await
		.map_err(|_| {
			aidash_application::Error::Port(Box::new(RunnerTimeout {
				route: path.to_owned(),
				seconds: deadline.as_secs(),
			}))
		})??;
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
	/// Stop a Background process through `path` and wait until the Runner shows
	/// it ended: a stop route answers `stopping` as soon as it signals the
	/// process, and a caller that took that as the end would drop the process's
	/// state (a preview's hold, a plugin's data) while it may still run.
	async fn stop_confirmed(&self, path: &str, operation_id: &str) -> Result<()> {
		let mut reply: CommandResult = self
			.pod_call(path, &json!({"operation_id":operation_id}))
			.await?;
		let deadline = tokio::time::Instant::now() + STOP_CONFIRMATION;
		loop {
			if matches!(
				reply.status.as_str(),
				"completed" | "stopped" | "cancelled" | "timed_out"
			) {
				return Ok(());
			}
			if tokio::time::Instant::now() >= deadline {
				return Err(ExecutionPodError::Remote {
					code: ExecutionPodErrorCode::TerminationUnconfirmed,
					message: format!("the process is still {}", reply.status),
				});
			}
			tokio::time::sleep(STOP_POLL).await;
			reply = self.result(operation_id).await?;
		}
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
	async fn refuse_command(
		&self,
		operation_id: &str,
		scratch_space_id: &str,
		run_id: &str,
	) -> Result<CommandRefusal> {
		self.pod_call(
			"/v2/commands/refuse",
			&json!({"operation_id":operation_id,"scratch_space_id":scratch_space_id,"run_id":run_id}),
		)
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
	async fn stop_command(&self, operation_id: &str) -> Result<CommandResult> {
		self.pod_call("/v2/commands/stop", &json!({"operation_id":operation_id}))
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
	async fn stop_run(
		&self,
		scratch_space_id: &str,
		run_id: &str,
		reason: RunStopReason,
	) -> Result<Vec<String>> {
		#[derive(serde::Deserialize)]
		struct Stopped {
			stopped: Vec<String>,
		}
		let result: Stopped = self
			.pod_call(
				"/v2/runs/stop",
				&json!({"scratch_space_id":scratch_space_id,"run_id":run_id,"reason":reason}),
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
		self.stop_confirmed("/v2/previews/stop", operation_id).await
	}
	async fn mcp_stop(&self, operation_id: &str) -> Result<()> {
		self.stop_confirmed("/v2/mcp/stop", operation_id).await
	}
	async fn mcp_stop_run(&self, scratch_space_id: &str, run_id: &str) -> Result<()> {
		let _: Value = self
			.pod_call(
				"/v2/mcp/stop_run",
				&json!({"scratch_space_id":scratch_space_id,"run_id":run_id}),
			)
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

#[cfg(test)]
mod timeout_tests {
	use super::*;

	#[test]
	fn each_pod_route_uses_its_required_wait_limit() {
		for (path, body, seconds) in [
			("/v2/previews/start", json!({}), 75),
			("/v2/mcp/start", json!({}), 315),
			("/v2/mcp/call", json!({"timeout_seconds": 42}), 57),
			("/v2/mcp/call", json!({"timeout_seconds": 0}), 15),
			("/v2/runs/stop", json!({}), 75),
			("/v2/mcp/stop_run", json!({}), 75),
			("/v2/runs/stop_sessions", json!({}), 75),
			("/v2/python/start", json!({}), 60),
			("/v2/python/cell", json!({}), 60),
			("/v2/python/stop", json!({}), 75),
			("/v2/commands/submit", json!({}), 60),
			("/v2/commands/refuse", json!({}), 60),
			("/v2/pods/ensure", json!({}), 1900),
		] {
			assert_eq!(
				pod_timeout(path, &body),
				Duration::from_secs(seconds),
				"{path}"
			);
		}
	}
}
