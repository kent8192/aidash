use super::*;
use crate::ports::capabilities::execution_pod::{
	BackgroundProcesses, ChangeLogChunk, CommandRefusal, CommandResult, EgressCredential,
	ExecutionPodRuntime, ExecutionPodSettings, FileChunk, FileManifestResult, FilePatchRequest,
	FilePatchResult, FileSearchRequest, FileSearchResult, FileWriteResult, LogChunk, LogRequest,
	ManifestFile, McpCallResult, McpSession, PodReceipt, PreviewStartRequest, PreviewTarget,
	PythonSession, PythonStartRequest, Result as PodResult,
};
use aidash_domain::agent_plugin::{Command, Cwd, Executable};
use std::collections::VecDeque;

type PodFiles = BTreeMap<String, (Vec<u8>, Option<u32>)>;

/// A Pod whose files live in a map, whose commands succeed, and whose MCP server answers from
/// a script.
#[derive(Default)]
struct Pod {
	calls: Mutex<Vec<String>>,
	commands: Mutex<Vec<String>>,
	files: Mutex<PodFiles>,
	starts: Mutex<Vec<McpStartRequest>>,
	messages: Mutex<Vec<Value>>,
	/// Answers to `tools/call`, in order; anything else answers from `tools`.
	call_answers: Mutex<VecDeque<PodResult<Value>>>,
	/// The listed tools, as the server would send them.
	tools: Mutex<Value>,
	stopped: Mutex<Vec<String>>,
	/// How the next MCP start ends: `None` answers, `Some(None)` never answers, and
	/// `Some(Some(error))` fails after the Runner may have started the process.
	start_outcome: Mutex<Option<Option<ExecutionPodError>>>,
	/// Answers to refusals, in order; none left answers with a transport error.
	refusals: Mutex<VecDeque<CommandRefusal>>,
	refused: Mutex<Vec<String>>,
	/// Runs whose MCP sessions the Runner was asked to stop.
	run_stops: Mutex<Vec<String>>,
}

impl Pod {
	fn new(tools: Value) -> Arc<Self> {
		Arc::new(Self {
			tools: Mutex::new(tools),
			..Self::default()
		})
	}
	fn call(&self, name: &str) {
		self.calls.lock().unwrap().push(name.into());
	}
	fn calls(&self) -> Vec<String> {
		self.calls.lock().unwrap().clone()
	}
	fn methods(&self) -> Vec<String> {
		self.messages
			.lock()
			.unwrap()
			.iter()
			.map(|message| message["method"].as_str().unwrap_or_default().to_owned())
			.collect()
	}
	fn answer(&self, answer: PodResult<Value>) {
		self.call_answers.lock().unwrap().push_back(answer);
	}
}

#[async_trait]
impl ExecutionPodRuntime for Pod {
	async fn ensure(&self, _: &ExecutionPodSettings) -> PodResult<PodReceipt> {
		self.call("ensure");
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
		assert_eq!(request.proxy_credential, None);
		self.commands.lock().unwrap().push(request.code.clone());
		Ok(CommandResult {
			operation_id: request.operation_id.clone(),
			status: "completed".into(),
			scratch_space_id: None,
			run_id: None,
			background: None,
			outcome: None,
			exit_code: Some(0),
			stdout: None,
			stderr: None,
			truncated: None,
			error_code: None,
			message: None,
			python_result: None,
			left_running: None,
			session_replaced: None,
			ended: None,
		})
	}
	async fn result(&self, _: &str) -> PodResult<CommandResult> {
		unimplemented!()
	}
	async fn cancel_waiting(&self, _: &str, _: &str) -> PodResult<()> {
		unimplemented!()
	}
	async fn backgrounds(&self, _: &str) -> PodResult<BackgroundProcesses> {
		unimplemented!()
	}
	async fn stop_background(&self, _: &str) -> PodResult<CommandResult> {
		unimplemented!()
	}
	async fn python_start(&self, _: &PythonStartRequest) -> PodResult<PythonSession> {
		unimplemented!()
	}
	async fn stop_run(&self, _: &str, _: &str) -> PodResult<Vec<String>> {
		unimplemented!()
	}
	async fn background_log(&self, _: &LogRequest) -> PodResult<LogChunk> {
		unimplemented!()
	}
	async fn file_read(&self, request: &FileReadRequest) -> PodResult<FileChunk> {
		let files = self.files.lock().unwrap();
		let (bytes, _) = files.get(&request.path).expect("file to read");
		let start = usize::try_from(request.offset.unwrap_or(0)).unwrap();
		let end = (start + request.limit as usize).min(bytes.len());
		Ok(FileChunk {
			data: STANDARD.encode(&bytes[start..end]),
			size: bytes.len() as u64,
			next_offset: end as u64,
			eof: end == bytes.len(),
		})
	}
	async fn file_search(&self, _: &FileSearchRequest) -> PodResult<FileSearchResult> {
		unimplemented!()
	}
	async fn file_patch(&self, _: &FilePatchRequest) -> PodResult<FilePatchResult> {
		unimplemented!()
	}
	async fn file_write(&self, request: &FileWriteRequest) -> PodResult<FileWriteResult> {
		assert!(request.final_chunk);
		let bytes = STANDARD.decode(&request.data).unwrap();
		let size = bytes.len() as u64;
		self.files
			.lock()
			.unwrap()
			.insert(request.path.clone(), (bytes, request.mode));
		Ok(FileWriteResult { size })
	}
	async fn file_manifest(&self, request: &FileManifestRequest) -> PodResult<FileManifestResult> {
		self.call("manifest");
		let prefix = format!("{}/", request.root);
		Ok(FileManifestResult {
			files: self
				.files
				.lock()
				.unwrap()
				.iter()
				.filter(|(path, _)| path.starts_with(&prefix))
				.map(|(path, (bytes, _))| ManifestFile {
					path: path.clone(),
					size: bytes.len() as u64,
					sha256: hex(&Sha256::digest(bytes)),
				})
				.collect(),
			skipped_symlinks: Vec::new(),
			truncated: false,
		})
	}
	async fn preview_start(&self, _: &PreviewStartRequest) -> PodResult<PreviewTarget> {
		unimplemented!()
	}
	async fn preview_stop(&self, _: &str) -> PodResult<()> {
		unimplemented!()
	}
	async fn refuse_command(
		&self,
		operation_id: &str,
		scratch_space_id: &str,
		run_id: &str,
	) -> PodResult<CommandRefusal> {
		self.call("refuse");
		assert_eq!(scratch_space_id, "space");
		assert_eq!(run_id, run().to_string());
		self.refused.lock().unwrap().push(operation_id.into());
		self.refusals
			.lock()
			.unwrap()
			.pop_front()
			.ok_or_else(|| ExecutionPodError::Transport(Error::External("runner offline".into())))
	}
	async fn mcp_start(&self, request: &McpStartRequest) -> PodResult<McpSession> {
		self.call("mcp_start");
		self.starts.lock().unwrap().push(request.clone());
		let outcome = self.start_outcome.lock().unwrap().take();
		match outcome {
			None => Ok(McpSession {
				operation_id: request.operation_id.clone(),
			}),
			Some(None) => std::future::pending().await,
			Some(Some(error)) => Err(error),
		}
	}
	async fn mcp_call(&self, request: &McpCallRequest) -> PodResult<McpCallResult> {
		let message = request.message.clone();
		self.messages.lock().unwrap().push(message.clone());
		let id = message["id"].clone();
		let answer = match message["method"].as_str() {
			Some("initialize") => json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}),
			Some("notifications/initialized") => {
				return Ok(McpCallResult { message: json!({}) });
			}
			Some("tools/list") => json!({"tools": self.tools.lock().unwrap().clone()}),
			Some("tools/call") => {
				assert_eq!(request.timeout_seconds, CALL_SECONDS);
				match self.call_answers.lock().unwrap().pop_front() {
					Some(Ok(result)) => result,
					Some(Err(error)) => return Err(error),
					None => json!({"content": [{"type": "text", "text": "ok"}]}),
				}
			}
			other => panic!("unexpected method {other:?}"),
		};
		Ok(McpCallResult {
			message: json!({"jsonrpc": "2.0", "id": id, "result": answer}),
		})
	}
	async fn mcp_stop(&self, operation_id: &str) -> PodResult<()> {
		self.call("mcp_stop");
		self.stopped.lock().unwrap().push(operation_id.into());
		Ok(())
	}
	async fn mcp_stop_run(&self, scratch_space_id: &str, run_id: &str) -> PodResult<()> {
		self.call("mcp_stop_run");
		assert_eq!(scratch_space_id, "space");
		self.run_stops.lock().unwrap().push(run_id.into());
		Ok(())
	}
	async fn python_cell(&self, _: &CommandRequest) -> PodResult<CommandResult> {
		unimplemented!()
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

struct Host {
	pod: Arc<Pod>,
	approved: Mutex<bool>,
	saved: Mutex<Option<Vec<PluginFile>>>,
	saves: Mutex<u32>,
	not_saved: Mutex<Vec<String>>,
	refusal: Mutex<Option<String>>,
	forbidden: Mutex<bool>,
}

impl Host {
	fn new(pod: &Arc<Pod>) -> Arc<Self> {
		Arc::new(Self {
			pod: pod.clone(),
			approved: Mutex::new(true),
			saved: Mutex::new(None),
			saves: Mutex::new(0),
			not_saved: Mutex::new(Vec::new()),
			refusal: Mutex::new(None),
			forbidden: Mutex::new(false),
		})
	}
}

#[async_trait]
impl StdioPluginHost for Host {
	async fn pod(&self) -> Result<Option<StdioPod>> {
		Ok(Some(StdioPod {
			runtime: self.pod.clone(),
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
		}))
	}
	async fn approved(&self) -> Result<bool> {
		Ok(*self.approved.lock().unwrap())
	}
	async fn admit_call(&self) -> Result<()> {
		if *self.forbidden.lock().unwrap() {
			return Err(Error::Forbidden);
		}
		match self.refusal.lock().unwrap().clone() {
			Some(code) => Err(Error::Refused(code)),
			None => Ok(()),
		}
	}
	async fn plugin_files(&self, _: &str) -> Result<Vec<PluginFile>> {
		Ok(vec![
			PluginFile {
				path: "bin/server.js".into(),
				bytes: b"console.log(1)".to_vec(),
			},
			PluginFile {
				path: "plugin.json".into(),
				bytes: b"{}".to_vec(),
			},
		])
	}
	async fn load_data(&self, _: &PluginName) -> Result<Option<Vec<PluginFile>>> {
		Ok(self.saved.lock().unwrap().clone())
	}
	async fn save_data(&self, _: &PluginName, files: Vec<PluginFile>) -> Result<()> {
		if *self.forbidden.lock().unwrap() {
			return Err(Error::Forbidden);
		}
		*self.saves.lock().unwrap() += 1;
		*self.saved.lock().unwrap() = Some(files);
		Ok(())
	}
	async fn data_not_saved(&self, _: &PluginName, cause: &str) {
		self.not_saved.lock().unwrap().push(cause.into());
	}
}

fn run() -> Uuid {
	Uuid::from_u128(9)
}

fn server() -> PluginServer {
	PluginServer {
		plugin: PluginName::try_from("acme-tools".to_owned()).unwrap(),
		digest: "sha256:abc".into(),
		server: ServerName::try_from("local".to_owned()).unwrap(),
	}
}

fn config(command: Command) -> StdioServer {
	StdioServer {
		command,
		args: vec!["--data".into(), "${PLUGIN_DATA}".into()],
		env: BTreeMap::from([("MODE".to_owned(), "test".to_owned())]),
		cwd: Cwd::PluginRoot,
	}
}

fn node() -> StdioServer {
	config(Command::PluginRelative("bin/server.js".into()))
}

fn schema() -> Value {
	json!({"type": "object"})
}

fn listed() -> Value {
	json!([{"name": "search", "description": "Searches.", "inputSchema": schema()}])
}

fn digest() -> String {
	aidash_domain::agent_plugin::schema_digest(&schema())
}

fn setup() -> (
	Arc<Pod>,
	Arc<Host>,
	Arc<StdioSessions>,
	Arc<dyn StdioPluginSessions>,
) {
	let pod = Pod::new(listed());
	let host = Host::new(&pod);
	let sessions = Arc::new(StdioSessions::default());
	let bound = sessions.bind(run(), host.clone());
	(pod, host, sessions, bound)
}

#[tokio::test]
async fn listing_starts_the_session_in_the_pod_with_the_run_credential_and_its_plugin_tree() {
	// Arrange
	let (pod, _, _, sessions) = setup();

	// Act
	let tools = sessions.list(run(), &server(), &node()).await.unwrap();

	// Assert
	assert_eq!(tools.len(), 1);
	assert_eq!(tools[0].name, "search");
	assert_eq!(pod.calls(), ["ensure", "execute", "mcp_start"]);
	let start = pod.starts.lock().unwrap()[0].clone();
	let data = format!("/scratch/.aidash/plugin-data/{}/acme-tools", run());
	assert_eq!(start.plugin_data, data);
	assert_eq!(start.run_id, run().to_string());
	assert_eq!(start.install, "");
	assert_eq!(
		start.proxy_credential,
		Some(EgressCredential("run-credential".into()))
	);
	assert_eq!(start.env["MODE"], "test");
	assert_eq!(
		&start.command[5..],
		[
			format!(
				"/scratch/.aidash/plugins/{}/sha256-abc/bin/server.js",
				run()
			),
			"--data".to_owned(),
			data,
		]
	);
	let files = pod.files.lock().unwrap();
	let root = format!(".aidash/plugins/{}/sha256-abc", run());
	assert_eq!(files[&format!("{root}/bin/server.js")].1, Some(0o755));
	assert_eq!(files[&format!("{root}/plugin.json")].1, Some(0o644));
	let commands = pod.commands.lock().unwrap();
	assert_eq!(
		commands[0],
		format!("rm -rf -- '{root}' && mkdir -p -- '{root}' '{root}/bin'")
	);
	assert_eq!(
		pod.methods(),
		["initialize", "notifications/initialized", "tools/list"]
	);
}

#[tokio::test]
async fn calls_reuse_the_listed_session_and_map_error_results() {
	// Arrange
	let (pod, _, _, sessions) = setup();
	sessions.list(run(), &server(), &node()).await.unwrap();
	pod.answer(Ok(json!({"content": [{"type": "text", "text": "ok"}]})));
	pod.answer(Ok(
		json!({"isError": true, "content": [{"type": "text", "text": "no"}]}),
	));

	// Act
	let ok = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({"q": "a"}),
			"k1",
		)
		.await
		.unwrap();
	let failed = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k2",
		)
		.await
		.unwrap();

	// Assert
	assert_eq!(ok, json!({"content": [{"type": "text", "text": "ok"}]}));
	assert_eq!(
		failed,
		json!({"is_error": true, "content": [{"type": "text", "text": "no"}]})
	);
	assert_eq!(pod.starts.lock().unwrap().len(), 1);
	let messages = pod.messages.lock().unwrap();
	assert_eq!(
		messages[3]["params"],
		json!({"name": "search", "arguments": {"q": "a"}})
	);
}

#[tokio::test]
async fn a_lost_answer_is_uncertain_and_the_next_call_starts_a_new_session_without_resending() {
	// Arrange
	let (pod, _, _, sessions) = setup();
	pod.answer(Err(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::Unknown("mcp_call_failed".into()),
		message: "MCP call outcome is unknown".into(),
	}));

	// Act
	let lost = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k1",
		)
		.await;
	let next = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k2",
		)
		.await;

	// Assert
	assert!(matches!(lost, Err(Error::External(_))));
	assert!(next.is_ok());
	assert_eq!(pod.starts.lock().unwrap().len(), 2);
	let calls = pod
		.methods()
		.iter()
		.filter(|method| *method == "tools/call")
		.count();
	assert_eq!(calls, 2);
}

#[tokio::test]
async fn a_session_that_is_gone_proves_the_call_was_not_delivered() {
	// Arrange
	let (pod, _, _, sessions) = setup();
	pod.answer(Err(ExecutionPodError::Remote {
		code: ExecutionPodErrorCode::Unknown("mcp_unavailable".into()),
		message: "MCP session is not running".into(),
	}));

	// Act
	let result = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k1",
		)
		.await;

	// Assert
	assert!(matches!(result, Err(Error::ToolNotDelivered { .. })));
}

#[tokio::test]
async fn a_new_session_refuses_a_tool_whose_schema_moved() {
	// Arrange
	let (pod, _, _, sessions) = setup();
	*pod.tools.lock().unwrap() =
		json!([{"name": "search", "inputSchema": {"type": "object", "required": ["q"]}}]);

	// Act
	let result = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k1",
		)
		.await;

	// Assert
	assert!(
		matches!(result, Err(Error::Invalid(message)) if message.starts_with("CAPABILITY_CHANGED"))
	);
	assert!(!pod.methods().contains(&"tools/call".to_owned()));
}

#[tokio::test]
async fn a_retried_call_of_a_tool_whose_schema_moved_is_refused_again() {
	// Arrange: the first call started and kept the session.
	let (pod, _, _, sessions) = setup();
	*pod.tools.lock().unwrap() =
		json!([{"name": "search", "inputSchema": {"type": "object", "required": ["q"]}}]);
	let (server, node, digest) = (server(), node(), digest());
	let call = || sessions.call(run(), &server, &node, "search", &digest, json!({}), "k1");
	assert!(call().await.is_err());

	// Act
	let retried = call().await;

	// Assert
	assert!(
		matches!(retried, Err(Error::Invalid(message)) if message.starts_with("CAPABILITY_CHANGED"))
	);
	assert!(!pod.methods().contains(&"tools/call".to_owned()));
}

#[tokio::test]
async fn a_second_server_of_the_plugin_keeps_the_tree_the_first_runs_from() {
	// Arrange
	let (pod, _, _, sessions) = setup();
	sessions.list(run(), &server(), &node()).await.unwrap();
	let second = PluginServer {
		server: ServerName::try_from("other".to_owned()).unwrap(),
		..server()
	};

	// Act
	sessions.list(run(), &second, &node()).await.unwrap();

	// Assert: only the first start removed and wrote the tree.
	let root = format!(".aidash/plugins/{}/sha256-abc", run());
	let commands = pod.commands.lock().unwrap();
	assert_eq!(
		commands
			.iter()
			.filter(|command| command.contains(&format!("'{root}'")))
			.count(),
		1,
		"{commands:?}"
	);
}

#[tokio::test]
async fn without_the_tenants_approval_nothing_starts() {
	// Arrange
	let (pod, host, _, sessions) = setup();
	*host.approved.lock().unwrap() = false;

	// Act
	let listed = sessions.list(run(), &server(), &node()).await;
	let called = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k1",
		)
		.await;

	// Assert
	assert!(matches!(listed, Err(Error::Invalid(cause)) if cause == NOT_APPROVED));
	assert!(
		matches!(called, Err(Error::Invalid(message)) if message.starts_with("MCP_STDIO_NOT_APPROVED"))
	);
	assert!(pod.calls().is_empty());
}

#[tokio::test]
async fn a_package_fetcher_waits_for_a_pinned_lockfile() {
	// Arrange
	let (pod, _, _, sessions) = setup();

	// Act
	let listed = sessions
		.list(run(), &server(), &config(Command::Bare(Executable::Npx)))
		.await;

	// Assert
	assert!(
		matches!(listed, Err(Error::Invalid(cause)) if cause == stdio_session::DEPENDENCIES_NOT_PINNED)
	);
	assert!(pod.calls().is_empty());
}

#[tokio::test]
async fn closing_stops_the_sessions_and_saves_checked_plugin_data_for_the_next_run() {
	// Arrange
	let (pod, host, sessions, bound) = setup();
	bound.list(run(), &server(), &node()).await.unwrap();
	let data = stdio_session::plugin_data(run(), &server().plugin);
	pod.files.lock().unwrap().insert(
		format!("{data}/cache/state.json"),
		(b"{\"n\":1}".to_vec(), None),
	);

	// Act
	sessions.close(run()).await;
	let next = Uuid::from_u128(10);
	sessions
		.bind(next, host.clone())
		.list(next, &server(), &node())
		.await
		.unwrap();

	// Assert
	assert_eq!(pod.stopped.lock().unwrap().len(), 1);
	assert_eq!(
		host.saved.lock().unwrap().clone().unwrap(),
		[PluginFile {
			path: "cache/state.json".into(),
			bytes: b"{\"n\":1}".to_vec(),
		}]
	);
	let restored = stdio_session::plugin_data(next, &server().plugin);
	let commands = pod.commands.lock().unwrap().clone();
	assert_eq!(commands[1], format!("rm -rf -- '{data}'"));
	// The closed Run's plugin trees go too.
	assert_eq!(
		commands[2],
		format!("rm -rf -- '{}'", stdio_session::run_plugins(run()))
	);
	assert!(commands[3].contains(&format!("'{restored}'")));
	assert_eq!(
		pod.files.lock().unwrap()[&format!("{restored}/cache/state.json")].0,
		b"{\"n\":1}"
	);
	assert!(host.not_saved.lock().unwrap().is_empty());
}

#[tokio::test]
async fn data_over_the_cap_keeps_the_previous_version() {
	// Arrange
	let (pod, host, sessions, bound) = setup();
	bound.list(run(), &server(), &node()).await.unwrap();
	let data = stdio_session::plugin_data(run(), &server().plugin);
	let big = vec![0u8; usize::try_from(stdio_session::MAX_PLUGIN_DATA_BYTES).unwrap() + 1];
	pod.files
		.lock()
		.unwrap()
		.insert(format!("{data}/big"), (big, None));

	// Act
	sessions.close(run()).await;

	// Assert
	let commands = pod.commands.lock().unwrap().clone();
	assert_eq!(
		commands[1],
		format!("rm -rf -- '{data}'"),
		"the live area goes"
	);
	assert_eq!(*host.saves.lock().unwrap(), 0);
	assert_eq!(
		host.not_saved.lock().unwrap().clone(),
		[stdio_session::DATA_TOO_LARGE]
	);
}

#[tokio::test]
async fn a_call_the_shell_gates_refuse_never_reaches_the_server() {
	// Arrange
	let (pod, host, _, sessions) = setup();
	*host.refusal.lock().unwrap() = Some("CAPABILITIES_DISABLED".into());

	// Act
	let result = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k1",
		)
		.await;

	// Assert
	assert!(matches!(result, Err(Error::Refused(code)) if code == "CAPABILITIES_DISABLED"));
	assert!(pod.calls().is_empty());
}

#[tokio::test]
async fn a_principal_refused_tool_invoke_cannot_make_a_stdio_call() {
	// Arrange: the host's `tool.invoke` decision for the Run's principals refuses.
	let (pod, host, _, sessions) = setup();
	sessions.list(run(), &server(), &node()).await.unwrap();
	*host.forbidden.lock().unwrap() = true;

	// Act
	let result = sessions
		.call(
			run(),
			&server(),
			&node(),
			"search",
			&digest(),
			json!({}),
			"k1",
		)
		.await;

	// Assert
	assert!(matches!(result, Err(Error::Forbidden)), "{result:?}");
	assert!(!pod.methods().contains(&"tools/call".to_owned()));
}

#[tokio::test]
async fn a_listing_the_gate_refuses_starts_no_process() {
	// Arrange
	let (pod, host, _, sessions) = setup();
	*host.forbidden.lock().unwrap() = true;

	// Act
	let listed = sessions.list(run(), &server(), &node()).await;

	// Assert
	assert!(
		matches!(&listed, Err(Error::Invalid(cause)) if cause == REFUSED),
		"{listed:?}"
	);
	assert!(pod.calls().is_empty());
}

#[tokio::test]
async fn a_listing_refused_by_the_usage_gate_starts_no_process() {
	let (pod, host, _, sessions) = setup();
	*host.refusal.lock().unwrap() = Some("SANDBOX_LIMIT".into());

	let listed = sessions.list(run(), &server(), &node()).await;

	assert!(
		matches!(&listed, Err(Error::Invalid(cause)) if cause == "sandbox_limit"),
		"{listed:?}"
	);
	assert!(pod.calls().is_empty());
}

#[tokio::test]
async fn a_run_that_lost_its_authority_keeps_the_saved_version_and_its_live_area_goes() {
	// Arrange
	let (pod, host, sessions, bound) = setup();
	bound.list(run(), &server(), &node()).await.unwrap();
	let data = stdio_session::plugin_data(run(), &server().plugin);
	pod.files
		.lock()
		.unwrap()
		.insert(format!("{data}/state"), (b"new".to_vec(), None));
	*host.saved.lock().unwrap() = Some(vec![PluginFile {
		path: "state".into(),
		bytes: b"old".to_vec(),
	}]);
	*host.forbidden.lock().unwrap() = true;

	// Act
	sessions.close(run()).await;

	// Assert
	assert_eq!(host.saved.lock().unwrap().clone().unwrap()[0].bytes, b"old");
	assert_eq!(host.not_saved.lock().unwrap().len(), 1);
	let commands = pod.commands.lock().unwrap().clone();
	assert_eq!(commands[commands.len() - 2], format!("rm -rf -- '{data}'"));
}

#[tokio::test]
async fn a_server_whose_session_never_passed_the_gate_saves_nothing() {
	let (pod, host, sessions, bound) = setup();
	*host.forbidden.lock().unwrap() = true;
	let _ = bound.list(run(), &server(), &node()).await;

	sessions.close(run()).await;

	assert_eq!(*host.saves.lock().unwrap(), 0);
	assert!(host.not_saved.lock().unwrap().is_empty());
	assert!(pod.calls().is_empty());
}

#[tokio::test]
async fn a_start_dropped_at_its_time_limit_is_fenced_and_stopped_before_the_data_is_saved() {
	// Arrange: the Runner never answers the start, and the caller's time limit drops it.
	let (pod, host, sessions, bound) = setup();
	*pod.start_outcome.lock().unwrap() = Some(None);
	pod.refusals.lock().unwrap().push_back(CommandRefusal {
		refused: false,
		status: Some("running".into()),
	});
	let dropped = tokio::time::timeout(
		Duration::from_millis(50),
		bound.list(run(), &server(), &node()),
	)
	.await;
	assert!(dropped.is_err());
	let operation_id = pod.starts.lock().unwrap()[0].operation_id.clone();

	// Act
	sessions.close(run()).await;

	// Assert: the process the start may have launched is stopped by id and by Run before the
	// plugin data is read for saving or removed.
	assert_eq!(*pod.refused.lock().unwrap(), [operation_id.clone()]);
	assert_eq!(*pod.stopped.lock().unwrap(), [operation_id]);
	assert_eq!(*pod.run_stops.lock().unwrap(), [run().to_string()]);
	let calls = pod.calls();
	let position = |name: &str| calls.iter().position(|call| call == name).unwrap();
	assert!(position("mcp_stop_run") < position("manifest"), "{calls:?}");
	assert!(position("mcp_stop") < position("manifest"), "{calls:?}");
	assert_eq!(*host.saves.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_start_whose_answer_was_lost_blocks_the_next_start_until_it_is_accounted_for() {
	// Arrange: the start fails after the Runner may have launched the process, and the
	// Runner cannot be asked about it yet.
	let (pod, host, sessions, bound) = setup();
	*pod.start_outcome.lock().unwrap() = Some(Some(ExecutionPodError::Transport(Error::External(
		"connection reset".into(),
	))));

	// Act
	let lost = bound.list(run(), &server(), &node()).await;
	let blocked = bound.list(run(), &server(), &node()).await;
	sessions.close(run()).await;

	// Assert: no second process starts next to the first, and the Run's data stays in place.
	assert!(
		matches!(&lost, Err(Error::Invalid(cause)) if cause == START_FAILED),
		"{lost:?}"
	);
	assert!(
		matches!(&blocked, Err(Error::Invalid(cause)) if cause == START_FAILED),
		"{blocked:?}"
	);
	assert_eq!(pod.starts.lock().unwrap().len(), 1);
	assert_eq!(*host.saves.lock().unwrap(), 0);
	assert!(
		!pod.commands
			.lock()
			.unwrap()
			.iter()
			.any(|command| command.starts_with("rm -rf -- ") && !command.contains("mkdir"))
	);

	// Act: the Runner answers again; the id it never accepted is refused for good.
	pod.refusals.lock().unwrap().push_back(CommandRefusal {
		refused: true,
		status: None,
	});
	let tools = bound.list(run(), &server(), &node()).await.unwrap();

	// Assert
	assert_eq!(tools.len(), 1);
	assert_eq!(pod.starts.lock().unwrap().len(), 2);
	let first = pod.starts.lock().unwrap()[0].operation_id.clone();
	assert_eq!(pod.refused.lock().unwrap().last(), Some(&first));
}

#[tokio::test]
async fn a_call_waiting_while_the_run_closes_starts_a_fresh_session_after_the_close() {
	// Arrange
	let (pod, host, sessions, bound) = setup();
	bound.list(run(), &server(), &node()).await.unwrap();
	let state = sessions.runs.lock().unwrap()[&run()].clone();
	let held = state.lock().await;

	// Act: a close and a call both wait for the Run's lock; the close was first.
	let closing = tokio::spawn({
		let sessions = sessions.clone();
		async move { sessions.close(run()).await }
	});
	for _ in 0..16 {
		tokio::task::yield_now().await;
	}
	let calling = tokio::spawn({
		let bound = sessions.bind(run(), host.clone());
		async move {
			bound
				.call(
					run(),
					&server(),
					&node(),
					"search",
					&digest(),
					json!({}),
					"k1",
				)
				.await
		}
	});
	for _ in 0..16 {
		tokio::task::yield_now().await;
	}
	drop(held);
	closing.await.unwrap();
	calling.await.unwrap().unwrap();

	// Assert: the call did not run on the closed state; it started a new session, and only
	// after the close removed the Run's plugin trees, which the new session writes afresh.
	assert_eq!(pod.starts.lock().unwrap().len(), 2);
	assert_eq!(pod.stopped.lock().unwrap().len(), 1);
	let commands = pod.commands.lock().unwrap().clone();
	let removed = commands
		.iter()
		.position(|command| {
			*command == format!("rm -rf -- '{}'", stdio_session::run_plugins(run()))
		})
		.unwrap();
	let rewritten = commands
		.iter()
		.rposition(|command| command.contains("mkdir"))
		.unwrap();
	assert!(removed < rewritten, "{commands:?}");
	let fresh = sessions.runs.lock().unwrap()[&run()].clone();
	assert!(!Arc::ptr_eq(&fresh, &state));
	drop(bound);
}
