use super::*;
use crate::ports::capabilities::execution_pod::{
	BackgroundProcesses, ChangeLogChunk, CommandRefusal, EgressCredential, ExecutionPodRuntime,
	ExecutionPodSettings, FileChunk, FileManifestRequest, FileManifestResult, FilePatchRequest,
	FilePatchResult, FileReadRequest, FileSearchRequest, FileSearchResult, FileWriteRequest,
	FileWriteResult, LogChunk, LogRequest, ManifestFile, McpCallRequest, McpCallResult, McpSession,
	McpStartRequest, PodReceipt, PreviewStartRequest, PreviewTarget, PythonSession,
	PythonStartRequest, Result as PodResult,
};
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
};

/// Records every Runner call and answers submits and results from queues.
#[derive(Default)]
pub(crate) struct Runtime {
	pub(crate) calls: Mutex<Vec<String>>,
	pub(crate) requests: Mutex<Vec<CommandRequest>>,
	pub(crate) replies: Mutex<VecDeque<PodResult<CommandResult>>>,
	pub(crate) ensure_error: Mutex<Option<ExecutionPodError>>,
	/// Scratch Space files by path, which file reads, writes and manifests use.
	pub(crate) files: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
	/// Answers the next file call with `pod_unavailable` this many times.
	pub(crate) unavailable: Mutex<u32>,
	/// Answers the next file call with `file_helper_failed`: the exec failed on a running Pod.
	pub(crate) helper_failed: Mutex<bool>,
	pub(crate) writes: Mutex<Vec<FileWriteRequest>>,
	pub(crate) manifests: Mutex<Vec<FileManifestRequest>>,
	pub(crate) search: Mutex<Option<FileSearchResult>>,
	pub(crate) searches: Mutex<Vec<FileSearchRequest>>,
	pub(crate) patches: Mutex<Vec<FilePatchRequest>>,
	pub(crate) processes: Mutex<Vec<CommandResult>>,
	pub(crate) logs: Mutex<Vec<LogRequest>>,
	pub(crate) stopped: Mutex<Vec<String>>,
	pub(crate) previews: Mutex<Vec<PreviewStartRequest>>,
	/// The answer to a refusal; none answers with a transport error.
	pub(crate) refusal: Mutex<Option<CommandRefusal>>,
}
impl Runtime {
	pub(crate) fn reply(&self, reply: PodResult<CommandResult>) {
		self.replies.lock().unwrap().push_back(reply);
	}
	fn next(&self) -> PodResult<CommandResult> {
		self.replies
			.lock()
			.unwrap()
			.pop_front()
			.expect("an answer for this Runner call")
	}
	fn call(&self, name: &str) {
		self.calls.lock().unwrap().push(name.into());
	}
	pub(crate) fn calls(&self) -> Vec<String> {
		self.calls.lock().unwrap().clone()
	}
	fn available(&self) -> PodResult<()> {
		if std::mem::take(&mut *self.helper_failed.lock().unwrap()) {
			return Err(ExecutionPodError::Remote {
				code: ExecutionPodErrorCode::from("file_helper_failed"),
				message: "File helper failed; the call may have taken effect".into(),
			});
		}
		let mut left = self.unavailable.lock().unwrap();
		if *left > 0 {
			*left -= 1;
			return Err(ExecutionPodError::Remote {
				code: ExecutionPodErrorCode::PodUnavailable,
				message: "Execution Pod is not running".into(),
			});
		}
		Ok(())
	}
	pub(crate) fn file(&self, path: &str, bytes: &[u8]) {
		self.files
			.lock()
			.unwrap()
			.insert(path.into(), bytes.to_vec());
	}
}
#[async_trait::async_trait]
impl ExecutionPodRuntime for Runtime {
	async fn ensure(&self, _: &ExecutionPodSettings) -> PodResult<PodReceipt> {
		self.call("ensure");
		if let Some(error) = self.ensure_error.lock().unwrap().take() {
			return Err(error);
		}
		Ok(PodReceipt {
			protocol: "aidash-runner/2".into(),
			pod: "pod".into(),
			claim: "claim".into(),
			reset: false,
			image_changed: false,
		})
	}
	async fn execute(&self, request: &CommandRequest) -> PodResult<CommandResult> {
		self.call("execute");
		self.requests.lock().unwrap().push(request.clone());
		self.next()
	}
	async fn result(&self, _: &str) -> PodResult<CommandResult> {
		self.call("result");
		self.next()
	}
	async fn refuse_command(&self, _: &str, _: &str, _: &str) -> PodResult<CommandRefusal> {
		self.call("refuse_command");
		self.refusal
			.lock()
			.unwrap()
			.clone()
			.ok_or_else(|| ExecutionPodError::Transport(Error::External("unreachable".into())))
	}
	async fn cancel_waiting(&self, _: &str, _: &str) -> PodResult<()> {
		self.call("cancel_waiting");
		Ok(())
	}
	async fn stop_command(&self, operation_id: &str) -> PodResult<CommandResult> {
		self.call("stop_command");
		let mut stopped = reply("stopped");
		stopped.operation_id = operation_id.into();
		Ok(stopped)
	}
	async fn backgrounds(&self, _: &str) -> PodResult<BackgroundProcesses> {
		self.call("backgrounds");
		self.available()?;
		Ok(BackgroundProcesses {
			processes: self.processes.lock().unwrap().clone(),
		})
	}
	async fn stop_background(&self, operation_id: &str) -> PodResult<CommandResult> {
		self.call("stop_background");
		self.stopped.lock().unwrap().push(operation_id.into());
		let mut stopped = reply("stopped");
		stopped.operation_id = operation_id.into();
		Ok(stopped)
	}
	async fn python_start(&self, request: &PythonStartRequest) -> PodResult<PythonSession> {
		self.call("python_start");
		Ok(PythonSession {
			operation_id: "session".into(),
			scratch_space_id: request.scratch_space_id.clone(),
			run_id: request.run_id.clone(),
			socket_path: "/tmp/s".into(),
		})
	}
	async fn stop_run(
		&self,
		_: &str,
		_: &str,
		reason: crate::ports::capabilities::execution_pod::RunStopReason,
	) -> PodResult<Vec<String>> {
		match reason {
			crate::ports::capabilities::execution_pod::RunStopReason::RunCancelled => {
				self.call("run_cancelled")
			}
			crate::ports::capabilities::execution_pod::RunStopReason::RunAuthorityRevoked => {
				self.call("run_authority_revoked")
			}
		}
		Ok(vec![])
	}
	async fn background_log(&self, request: &LogRequest) -> PodResult<LogChunk> {
		self.call("background_log");
		self.logs.lock().unwrap().push(request.clone());
		Ok(LogChunk {
			data: STANDARD.encode("log line\n"),
			next_offset: 9,
			eof: true,
			size: 9,
		})
	}
	async fn file_read(&self, request: &FileReadRequest) -> PodResult<FileChunk> {
		self.call("file_read");
		self.available()?;
		let files = self.files.lock().unwrap();
		let bytes = files
			.get(&request.path)
			.ok_or_else(|| ExecutionPodError::Remote {
				code: ExecutionPodErrorCode::Unknown("file_error".into()),
				message: "No such file".into(),
			})?;
		let start = (request.offset.unwrap_or(0) as usize).min(bytes.len());
		let end = bytes.len().min(start + request.limit as usize);
		Ok(FileChunk {
			data: STANDARD.encode(&bytes[start..end]),
			size: bytes.len() as u64,
			next_offset: end as u64,
			eof: end == bytes.len(),
		})
	}
	async fn file_search(&self, request: &FileSearchRequest) -> PodResult<FileSearchResult> {
		self.call("file_search");
		self.available()?;
		self.searches.lock().unwrap().push(request.clone());
		Ok(self
			.search
			.lock()
			.unwrap()
			.clone()
			.unwrap_or(FileSearchResult {
				matches: vec![],
				truncated: false,
			}))
	}
	async fn file_patch(&self, request: &FilePatchRequest) -> PodResult<FilePatchResult> {
		self.call("file_patch");
		self.available()?;
		self.patches.lock().unwrap().push(request.clone());
		Ok(FilePatchResult {
			changed: vec!["a.txt".into()],
		})
	}
	async fn file_write(&self, request: &FileWriteRequest) -> PodResult<FileWriteResult> {
		self.call("file_write");
		self.available()?;
		self.writes.lock().unwrap().push(request.clone());
		let mut files = self.files.lock().unwrap();
		let file = files.entry(request.path.clone()).or_default();
		if request.offset == 0 {
			file.clear();
		}
		file.extend(STANDARD.decode(&request.data).unwrap());
		Ok(FileWriteResult {
			size: file.len() as u64,
		})
	}
	async fn file_manifest(&self, request: &FileManifestRequest) -> PodResult<FileManifestResult> {
		use sha2::Digest as _;
		self.call("file_manifest");
		self.available()?;
		self.manifests.lock().unwrap().push(request.clone());
		let files = self.files.lock().unwrap();
		let mut listed: Vec<ManifestFile> = files
			.iter()
			.filter(|(path, _)| {
				request.root == "."
					|| **path == request.root
					|| path.starts_with(&format!("{}/", request.root))
			})
			.map(|(path, bytes)| ManifestFile {
				path: path.clone(),
				size: bytes.len() as u64,
				sha256: format!("{:x}", sha2::Sha256::digest(bytes)),
			})
			.collect();
		let truncated = request
			.max_files
			.is_some_and(|limit| listed.len() > limit as usize);
		if let Some(limit) = request.max_files {
			listed.truncate(limit as usize);
		}
		Ok(FileManifestResult {
			files: listed,
			skipped_symlinks: vec![],
			truncated,
		})
	}
	async fn preview_start(&self, request: &PreviewStartRequest) -> PodResult<PreviewTarget> {
		self.call("preview_start");
		self.previews.lock().unwrap().push(request.clone());
		Ok(PreviewTarget {
			operation_id: request.operation_id.clone(),
			address: format!("10.0.0.7:{}", request.port),
		})
	}
	async fn preview_stop(&self, operation_id: &str) -> PodResult<()> {
		self.call("preview_stop");
		self.stopped.lock().unwrap().push(operation_id.into());
		Ok(())
	}
	async fn mcp_start(&self, _: &McpStartRequest) -> PodResult<McpSession> {
		unimplemented!()
	}
	async fn mcp_call(&self, _: &McpCallRequest) -> PodResult<McpCallResult> {
		unimplemented!()
	}
	async fn mcp_stop(&self, _: &str) -> PodResult<()> {
		unimplemented!()
	}
	async fn python_cell(&self, request: &CommandRequest) -> PodResult<CommandResult> {
		self.call("python_cell");
		self.requests.lock().unwrap().push(request.clone());
		self.next()
	}
	async fn python_stop(&self, _: &str, _: &str) -> PodResult<()> {
		unimplemented!()
	}
	async fn change_log(&self, _: &str, _: u64, _: u32) -> PodResult<ChangeLogChunk> {
		unimplemented!()
	}
	async fn viewer_heartbeat(&self, _: &str, _: &str) -> PodResult<()> {
		unimplemented!()
	}
	async fn reset(&self, _: &ExecutionPodSettings) -> PodResult<PodReceipt> {
		unimplemented!()
	}
	async fn stop(&self, _: &str) -> PodResult<()> {
		unimplemented!()
	}
	async fn delete(&self, _: &str) -> PodResult<()> {
		unimplemented!()
	}
}

pub(crate) fn context(runtime: Arc<Runtime>) -> PodContext {
	PodContext {
		runtime,
		settings: ExecutionPodSettings {
			scratch_space_id: "space".into(),
			image: "image".into(),
			cpu: 1,
			memory_bytes: 1,
			capacity_bytes: 1,
			docker_enabled: false,
			tenant_id: Some("acme".into()),
		},
		credential: Some(EgressCredential("run-credential".into())),
	}
}

struct Repository {
	operation: Mutex<PodOperation>,
	runtime: Arc<Runtime>,
	saves: Mutex<Vec<(String, String)>>,
	/// Fails `context` like a revoked Run or a deleted Scratch Space.
	context_error: Mutex<Option<Error>>,
}
#[async_trait::async_trait]
impl PodOperationRepository for Repository {
	async fn pending(&self) -> Result<Vec<Uuid>> {
		Ok(vec![self.operation.lock().unwrap().id])
	}
	async fn load(&self, _: Uuid) -> Result<PodOperation> {
		Ok(self.operation.lock().unwrap().clone())
	}
	async fn context(&self, _: &PodOperation) -> Result<PodContext> {
		if let Some(error) = self.context_error.lock().unwrap().take() {
			return Err(error);
		}
		Ok(context(self.runtime.clone()))
	}
	async fn save(&self, _: Uuid, expected: &str, state: &str, result: &Value) -> Result<bool> {
		let mut operation = self.operation.lock().unwrap();
		if operation.state != expected {
			return Ok(false);
		}
		self.saves
			.lock()
			.unwrap()
			.push((expected.into(), state.into()));
		operation.state = state.into();
		operation.result = result.clone();
		Ok(true)
	}
}

fn operation(kind: &str, state: &str) -> PodOperation {
	PodOperation {
		id: Uuid::from_u128(7),
		tenant: "acme".into(),
		run_id: Uuid::from_u128(9),
		kind: kind.into(),
		state: state.into(),
		input: json!({"command":"echo hi","seconds":60,"maximum_seconds":3600,"revision":4,
			"runtime":"pod","scratch_space_id":"space"}),
		result: json!({}),
		run_finished: false,
		run_cancelled: false,
	}
}

fn repository(operation: PodOperation) -> (Repository, Arc<Runtime>) {
	let runtime = Arc::new(Runtime::default());
	(
		Repository {
			operation: Mutex::new(operation),
			runtime: runtime.clone(),
			saves: Mutex::default(),
			context_error: Mutex::default(),
		},
		runtime,
	)
}

pub(crate) fn reply(status: &str) -> CommandResult {
	CommandResult {
		operation_id: Uuid::from_u128(7).to_string(),
		status: status.into(),
		scratch_space_id: Some("space".into()),
		run_id: Some(Uuid::from_u128(9).to_string()),
		background: Some(false),
		outcome: None,
		exit_code: None,
		stdout: None,
		stderr: None,
		truncated: None,
		error_code: None,
		message: None,
		python_result: None,
		pod_uid: None,
		session_id: None,
		left_running: None,
		stop_reason: None,
		output_tail: None,
		session_replaced: None,
	}
}

#[test]
fn a_command_at_its_limit_reports_the_timeout_that_keeps_the_scratch_space() {
	let operation = operation(SHELL, "running");
	let (state, result) = observe(&operation, &reply("timed_out"), 10.0);
	assert_eq!(state, "failed");
	let error = &result["error"];
	assert_eq!(error["code"], "COMMAND_TIMED_OUT");
	assert_eq!(error["details"]["limit_seconds"], 60);
	assert_eq!(error["details"]["revision"], 4);
	assert!(
		error["message"]
			.as_str()
			.unwrap()
			.contains("The Scratch Space keeps what the command wrote")
	);
	// The Runner's own status and code stay internal.
	assert!(!result.to_string().contains("\"timeout\""));
	assert_eq!(result["runner_acknowledged"], true);
	assert_eq!(result["termination_confirmed"], true);
}

#[test]
fn a_finished_command_keeps_its_output_order_exit_code_and_left_process() {
	let operation = operation(SHELL, "running");
	let mut finished = reply("completed");
	finished.stdout = Some(STANDARD.encode("out"));
	finished.stderr = Some(STANDARD.encode("err\n"));
	finished.exit_code = Some(3);
	finished.left_running = Some(format!("{}.left", operation.id));
	let (state, result) = observe(&operation, &finished, 10.0);
	assert_eq!(state, "completed");
	assert_eq!(
		STANDARD
			.decode(result["preview_bytes"].as_str().unwrap())
			.unwrap(),
		b"out\nerr\n"
	);
	assert_eq!(result["exit_code"], 3);
	assert_eq!(result["left_running"], format!("{}.left", operation.id));
	assert!(result["error"].is_null());
}

#[test]
fn a_running_command_shows_only_stdout_and_keeps_its_first_start() {
	let mut operation = operation(SHELL, "submitted");
	let mut running = reply("running");
	running.stdout = Some(STANDARD.encode("partial"));
	running.stderr = Some(STANDARD.encode("noise"));
	let (state, result) = observe(&operation, &running, 10.0);
	assert_eq!(state, "running");
	assert_eq!(result["started_at"], 10.0);
	assert_eq!(
		STANDARD
			.decode(result["preview_bytes"].as_str().unwrap())
			.unwrap(),
		b"partial"
	);
	operation.result = result;
	let (_, later) = observe(&operation, &running, 20.0);
	assert_eq!(later["started_at"], 10.0);
}

#[test]
fn a_cell_that_raised_fails_with_its_traceback_as_output() {
	let operation = operation(PYTHON, "running");
	let mut cell = reply("completed");
	cell.exit_code = Some(0);
	cell.python_result =
		Some(json!({"status":"failed","stdout":"Traceback ...","truncated":false}));
	let (state, result) = observe(&operation, &cell, 1.0);
	assert_eq!(state, "failed");
	assert_eq!(result["error"]["code"], "PYTHON_ERROR");
	assert_eq!(
		STANDARD
			.decode(result["preview_bytes"].as_str().unwrap())
			.unwrap(),
		b"Traceback ..."
	);
}

#[test]
fn a_cell_whose_interpreter_ended_fails_with_the_helpers_output() {
	let operation = operation(PYTHON, "running");
	let mut cell = reply("completed");
	cell.exit_code = Some(1);
	cell.stderr = Some(STANDARD.encode("EOFError: interpreter closed"));
	let (state, result) = observe(&operation, &cell, 1.0);
	assert_eq!(state, "failed");
	assert_eq!(result["error"]["code"], "PYTHON_SESSION_ENDED");
	assert_eq!(
		STANDARD
			.decode(result["preview_bytes"].as_str().unwrap())
			.unwrap(),
		b"EOFError: interpreter closed"
	);
}

#[test]
fn a_cell_in_a_restarted_interpreter_says_so_first() {
	let operation = operation(PYTHON, "running");
	let mut cell = reply("completed");
	cell.exit_code = Some(0);
	cell.session_replaced = Some(true);
	cell.python_result = Some(json!({"status":"completed","stdout":"1\n","truncated":false}));
	let (state, result) = observe(&operation, &cell, 1.0);
	assert_eq!(state, "completed");
	assert_eq!(result["session_reset"], true);
	let output = STANDARD
		.decode(result["preview_bytes"].as_str().unwrap())
		.unwrap();
	assert!(output.starts_with(b"[The Python session restarted"));
	assert!(output.ends_with(b"1\n"));
}

#[test]
fn a_lost_command_is_uncertain_and_never_confirmed() {
	let (state, result) = observe(&operation(SHELL, "running"), &reply("interrupted"), 1.0);
	assert_eq!(state, "uncertain");
	assert_eq!(result["termination_confirmed"], false);
	assert_eq!(result["error"]["code"], "EXECUTION_UNVERIFIED");
}

#[tokio::test]
async fn a_prepared_command_starts_the_pod_and_carries_the_run_credential() {
	let (repository, runtime) = repository(operation(SHELL, "prepared"));
	runtime.reply(Ok(reply("queued")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(runtime.calls(), ["ensure", "execute"]);
	let request = runtime.requests.lock().unwrap()[0].clone();
	assert_eq!(request.operation_id, Uuid::from_u128(7).to_string());
	assert_eq!(request.run_id, Uuid::from_u128(9).to_string());
	assert_eq!(request.code, "echo hi");
	assert_eq!(request.timeout_seconds, Some(60));
	assert!(!request.background);
	assert_eq!(
		request.proxy_credential,
		Some(EgressCredential("run-credential".into()))
	);
	assert_eq!(
		*repository.saves.lock().unwrap(),
		[
			("prepared".to_owned(), "submitted".to_owned()),
			("submitted".to_owned(), "submitted".to_owned())
		]
	);
	let saved = repository.operation.lock().unwrap().clone();
	assert!(saved.result.get("dispatch_pending").is_none());
	assert!(!saved.result.to_string().contains("run-credential"));
}

#[tokio::test]
async fn a_lost_submit_reply_is_resent_unchanged() {
	let (repository, runtime) = repository(operation(SHELL, "prepared"));
	runtime.reply(Err(ExecutionPodError::Transport(Error::External(
		"connection reset".into(),
	))));
	assert!(drive(&repository, Uuid::from_u128(7)).await.is_err());
	assert_eq!(repository.operation.lock().unwrap().state, "submitted");
	runtime.reply(Ok(reply("running")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	let requests = runtime.requests.lock().unwrap().clone();
	assert_eq!(requests.len(), 2);
	assert_eq!(requests[0], requests[1]);
	assert_eq!(repository.operation.lock().unwrap().state, "running");
}

#[tokio::test]
async fn a_refused_submit_fails_without_effects() {
	let (repository, runtime) = repository(operation(SHELL, "prepared"));
	runtime.reply(Err(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::QueueFull,
		message: "too many commands".into(),
	}));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.result["error"]["code"], "QUEUE_FULL");
	assert_eq!(saved.result["effects_may_have_occurred"], false);
}

#[tokio::test]
async fn a_pod_that_cannot_start_fails_the_command_before_it_is_sent() {
	let (repository, runtime) = repository(operation(SHELL, "prepared"));
	*runtime.ensure_error.lock().unwrap() = Some(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::StartupFailed,
		message: "image pull failed".into(),
	});
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(runtime.calls(), ["ensure"]);
	assert_eq!(repository.operation.lock().unwrap().state, "failed");
}

#[tokio::test]
async fn a_python_cell_carries_the_run_credential_and_returns_its_session() {
	let (repository, runtime) = repository(operation(PYTHON, "prepared"));
	let session_id = Uuid::from_u128(12);
	let mut accepted = reply("queued");
	accepted.session_id = Some(session_id);
	runtime.reply(Ok(accepted));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(runtime.calls(), ["ensure", "python_cell"]);
	let request = runtime.requests.lock().unwrap()[0].clone();
	assert_eq!(
		request.proxy_credential,
		Some(EgressCredential("run-credential".into()))
	);
	assert_eq!(request.run_id, Uuid::from_u128(9).to_string());
	assert_eq!(
		repository.operation.lock().unwrap().result["session_id"],
		json!(session_id)
	);
}

#[tokio::test]
async fn a_stale_python_cell_reports_reset_without_effects() {
	let mut operation = operation(PYTHON, "prepared");
	let expected = Uuid::from_u128(12);
	operation.input["expected_session_id"] = json!(expected);
	let (repository, runtime) = repository(operation);
	runtime.reply(Err(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::SessionReset,
		message: "Rebuild the Python session before retrying.".into(),
	}));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	let saved = repository.operation.lock().unwrap();
	assert_eq!(saved.state, "failed");
	assert_eq!(saved.result["error"]["code"], "SESSION_RESET");
	assert_eq!(saved.result["effects_may_have_occurred"], false);
	assert_eq!(saved.result["session_reset"], true);
	assert_eq!(
		serde_json::to_value(&runtime.requests.lock().unwrap()[0]).unwrap()["expected_session_id"],
		json!(expected)
	);
}

#[tokio::test]
async fn cancelling_stops_only_this_command_and_records_the_end() {
	let mut cancelling = operation(SHELL, "cancelling");
	cancelling.result = json!({"runner_acknowledged": true});
	let (repository, runtime) = repository(cancelling);
	runtime.reply(Ok(reply("stopped")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	// Not the Run-wide cancel: another command of the Run keeps running.
	assert_eq!(runtime.calls(), ["stop_command", "result"]);
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "cancelled");
	assert_eq!(saved.result["termination_confirmed"], true);
}

#[tokio::test]
async fn a_cancelling_command_of_a_revoked_run_ends_uncertain_instead_of_retrying() {
	let (repository, runtime) = repository(operation(SHELL, "cancelling"));
	*repository.context_error.lock().unwrap() = Some(Error::Forbidden);
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert!(runtime.calls().is_empty());
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "uncertain");
	assert_eq!(saved.result["termination_confirmed"], false);
	assert_eq!(saved.result["error"]["code"], "EXECUTION_UNVERIFIED");
}

#[tokio::test]
async fn a_finished_runs_command_stops_with_the_runs_foreground_work() {
	let mut running = operation(SHELL, "running");
	running.run_finished = true;
	let (repository, runtime) = repository(running);
	runtime.reply(Ok(reply("stopped")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(runtime.calls(), ["cancel_waiting", "result"]);
}

#[rstest::rstest]
#[case::completed(false, vec!["result"])]
#[case::cancelled(true, vec!["run_cancelled", "result"])]
#[tokio::test]
async fn background_settlement_distinguishes_run_completion_and_cancellation(
	#[case] cancelled: bool,
	#[case] calls: Vec<&str>,
) {
	let mut running = operation(SHELL, "running");
	running.run_finished = true;
	running.run_cancelled = cancelled;
	running.input["background"] = json!(true);
	running.input["notify_on_exit"] = json!(true);
	let (repository, runtime) = repository(running);
	let status = if cancelled { "stopped" } else { "completed" };
	let mut result = reply(status);
	result.exit_code = Some(if cancelled { -9 } else { 0 });
	result.stop_reason = cancelled.then(|| "run_cancelled".to_owned());
	result.output_tail = Some(STANDARD.encode(format!("{}exit", "x".repeat(4096))));
	runtime.reply(Ok(result));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(runtime.calls(), calls);
	let saved = repository.operation.lock().unwrap();
	let notice = &saved.result["background_exit"];
	assert_eq!(notice["status"], status);
	assert_eq!(notice["exit_code"], if cancelled { -9 } else { 0 });
	assert_eq!(
		notice["stop_reason"].as_str(),
		cancelled.then_some("run_cancelled")
	);
	assert_eq!(notice["output_tail"], format!("{}exit", "x".repeat(4092)));
}

#[tokio::test]
async fn a_command_still_running_while_cancelling_stays_cancelling() {
	let (repository, runtime) = repository(operation(SHELL, "cancelling"));
	runtime.reply(Ok(reply("running")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(repository.operation.lock().unwrap().state, "cancelling");
}

#[tokio::test]
async fn a_command_of_a_finished_run_that_never_started_is_cancelled_without_the_runner() {
	let mut prepared = operation(SHELL, "prepared");
	prepared.run_finished = true;
	let (repository, runtime) = repository(prepared);
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert!(runtime.calls().is_empty());
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "cancelled");
	assert_eq!(saved.result["effects_may_have_occurred"], false);
}

#[tokio::test]
async fn a_finished_operation_is_left_alone() {
	let (repository, runtime) = repository(operation(SHELL, "completed"));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert!(runtime.calls().is_empty());
	assert!(repository.saves.lock().unwrap().is_empty());
}

fn unsent() -> (Repository, Arc<Runtime>) {
	let mut cancelling = operation(SHELL, "cancelling");
	cancelling.result = json!({"dispatch_pending": true, "runner_acknowledged": true});
	let (repository, runtime) = repository(cancelling);
	runtime.reply(Err(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::CommandNotFound,
		message: "unknown".into(),
	}));
	(repository, runtime)
}

#[tokio::test]
async fn a_cancelled_command_the_runner_refused_never_ran() {
	let (repository, runtime) = unsent();
	*runtime.refusal.lock().unwrap() = Some(CommandRefusal {
		refused: true,
		status: None,
	});
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(
		runtime.calls(),
		["stop_command", "result", "refuse_command"]
	);
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "cancelled");
	assert_eq!(saved.result["termination_confirmed"], true);
	assert_eq!(saved.result["effects_may_have_occurred"], false);
	assert!(saved.result.get("dispatch_pending").is_none());
}

#[tokio::test]
async fn a_cancelled_command_the_runner_had_accepted_follows_its_state() {
	let (repository, runtime) = unsent();
	*runtime.refusal.lock().unwrap() = Some(CommandRefusal {
		refused: false,
		status: Some("running".into()),
	});
	runtime.reply(Ok(reply("running")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(repository.operation.lock().unwrap().state, "cancelling");
	let (repository, runtime) = unsent();
	*runtime.refusal.lock().unwrap() = Some(CommandRefusal {
		refused: false,
		status: Some("stopped".into()),
	});
	runtime.reply(Ok(reply("stopped")));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert_eq!(repository.operation.lock().unwrap().state, "cancelled");
}

#[tokio::test]
async fn a_cancelled_command_without_a_refusal_answer_stays_unproven() {
	let (repository, runtime) = unsent();
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	assert!(runtime.calls().contains(&"refuse_command".to_owned()));
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "uncertain");
	assert_eq!(saved.result["termination_confirmed"], false);
	assert!(saved.result.get("runner_acknowledged").is_none());
	assert!(saved.result.get("dispatch_pending").is_none());
}

#[tokio::test]
async fn a_submit_of_a_refused_id_ends_cancelled_without_effects() {
	let mut pending = operation(SHELL, "submitted");
	pending.result = json!({"dispatch_pending": true});
	let (repository, runtime) = repository(pending);
	runtime.reply(Err(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::CommandRefused,
		message: "refused".into(),
	}));
	drive(&repository, Uuid::from_u128(7)).await.unwrap();
	let saved = repository.operation.lock().unwrap().clone();
	assert_eq!(saved.state, "cancelled");
	assert_eq!(saved.result["effects_may_have_occurred"], false);
}

#[tokio::test]
async fn a_row_withdrawn_before_the_send_is_never_sent() {
	let mut pending = operation(SHELL, "submitted");
	pending.result = json!({"dispatch_pending": true});
	let (repository, runtime) = repository(pending.clone());
	repository.operation.lock().unwrap().state = "cancelling".into();
	let context = context(runtime.clone());
	submit(&repository, &pending, &context).await.unwrap();
	assert!(runtime.requests.lock().unwrap().is_empty());
}
