//! Sessions with the stdio MCP servers of Agent Plugins (ADR 0073 and its 2026-11-14
//! amendment). Each runs as a process in the Execution Pod of its Run's Scratch Space with the
//! Run credential of ADR 0136: one session per Run and server, started on first use, stopped
//! when the Run's sessions close. `${PLUGIN_DATA}` is restored before a plugin's first session in
//! the Run and saved, after a check, when they close.
use crate::{
	Error, Result, mcp_listing,
	ports::{
		capabilities::execution_pod::{
			CommandRefusal, CommandRequest, ExecutionPodError, ExecutionPodErrorCode,
			FileManifestRequest, FileReadRequest, FileWriteRequest, McpCallRequest,
			McpStartRequest,
		},
		tools::{ListFailure, PluginServer, StdioPluginHost, StdioPluginSessions, StdioPod},
	},
};
use aidash_domain::agent_plugin::{
	ListedTool, PluginFile, PluginName, STDIO_UNAVAILABLE, ServerName, StdioServer,
	stdio_session::{self, Launch},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
	collections::{BTreeMap, BTreeSet, HashMap},
	sync::{Arc, Mutex, Weak},
	time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, RwLock};
use uuid::Uuid;

/// How long one tool call may take in the server.
pub const CALL_SECONDS: u32 = 600;
/// How long a handshake exchange may take while starting a stdio process.
const SETUP_SECONDS: u32 = 60;
/// The Runner's limit on one chunk of a file read or write.
const CHUNK: usize = 1024 * 1024;
/// How long the command that prepares directories may run.
const PREPARE_SECONDS: u32 = 120;
const PREPARE_POLL: Duration = Duration::from_millis(250);
const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_ERROR_MESSAGE: usize = 4096;

/// Why a stdio server is not listed: Tenant has not approved the `mcp_stdio` group.
pub const NOT_APPROVED: &str = "mcp_stdio_not_approved";
pub const START_FAILED: &str = "start_failed";
/// The Run's principals may not use the `mcp_stdio` group, or the Run lost its authority.
pub const REFUSED: &str = "mcp_stdio_refused";
pub const INITIALIZE_FAILED: &str = "initialize_failed";
pub const INVALID_RESPONSE: &str = "invalid_response";

type SessionKey = (PluginName, String, ServerName);
type SessionSlot = Arc<AsyncMutex<Option<Session>>>;

/// The live sessions of every Run in this process, shared by every clone of the Store that
/// holds them.
#[derive(Default)]
pub struct StdioSessions {
	runs: Mutex<HashMap<Uuid, Arc<RunEntry>>>,
}

struct RunEntry {
	/// List and call share this guard. Run-wide stops and data removal require it exclusively.
	activity: Arc<RwLock<()>>,
	state: AsyncMutex<RunSessions>,
}

struct RunSessions {
	host: Arc<dyn StdioPluginHost>,
	pod: Option<StdioPod>,
	/// Plugins whose saved data this Run restored, so their data is saved at close.
	restored: BTreeSet<PluginName>,
	sessions: BTreeMap<SessionKey, SessionSlot>,
	/// Every process that may run in the Pod, recorded before sending its start.
	processes: BTreeMap<String, Process>,
	/// Set by [`StdioSessions::close`] once it has finished with this state; a caller that
	/// waited for the lock takes a fresh state instead.
	closed: bool,
}

struct Process {
	plugin_root: String,
	/// The session owns this marker from before startup until it leaves its slot. A dropped
	/// startup or unconfirmed stop leaves the operation id for the next recovery or close.
	owner: Weak<()>,
}

impl Process {
	fn abandoned(&self) -> bool {
		self.owner.strong_count() == 0
	}
}

impl RunSessions {
	fn has_abandoned(&self) -> bool {
		self.processes.values().any(Process::abandoned)
	}

	/// Under the exclusive activity guard, detach cached sessions but keep their operation ids
	/// until the Runner confirms they stopped.
	fn detach_sessions(&mut self) -> Vec<String> {
		let operations = self
			.processes
			.iter()
			.filter(|(_, process)| !process.abandoned())
			.map(|(operation, _)| operation.clone())
			.collect();
		self.sessions.clear();
		operations
	}
}

struct Session {
	operation_id: String,
	_process_owner: Arc<()>,
	/// Listed tools by name, with their schema digests.
	digests: BTreeMap<String, String>,
	listed: Vec<ListedTool>,
	next_id: u64,
}

impl Session {
	fn request(&mut self, method: &str, params: Value) -> Value {
		self.next_id += 1;
		json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
	}
}

/// Why a session could not start: a cause code for a finding or a not-delivered call.
struct Setup(String);
impl Setup {
	fn new(cause: &str) -> Self {
		Self(cause.to_owned())
	}
}
impl From<Error> for Setup {
	fn from(error: Error) -> Self {
		match error {
			Error::Invalid(cause) if !cause.contains(' ') => Self(cause),
			_ => Self::new(START_FAILED),
		}
	}
}
impl From<ListFailure> for Setup {
	fn from(failure: ListFailure) -> Self {
		Self(failure.to_string())
	}
}
impl From<ExecutionPodError> for Setup {
	fn from(error: ExecutionPodError) -> Self {
		match error {
			ExecutionPodError::Remote { code, .. } => Self(code.to_string()),
			ExecutionPodError::Transport(Error::Port(error))
				if error.is::<crate::ports::capabilities::execution_pod::RunnerTimeout>() =>
			{
				Self(format!("runner_timeout: {error}"))
			}
			ExecutionPodError::Transport(_) => Self::new(START_FAILED),
		}
	}
}

impl StdioSessions {
	/// The sessions of `run`, reached under the Run's current authority `host`.
	pub fn bind(
		self: &Arc<Self>,
		run: Uuid,
		host: Arc<dyn StdioPluginHost>,
	) -> Arc<dyn StdioPluginSessions> {
		Arc::new(RunStdio {
			sessions: self.clone(),
			run,
			host,
		})
	}

	fn run(&self, run: Uuid, host: &Arc<dyn StdioPluginHost>) -> Arc<RunEntry> {
		self.runs
			.lock()
			.expect("stdio sessions lock")
			.entry(run)
			.or_insert_with(|| {
				Arc::new(RunEntry {
					activity: Arc::new(RwLock::new(())),
					state: AsyncMutex::new(RunSessions {
						host: host.clone(),
						pod: None,
						restored: BTreeSet::new(),
						sessions: BTreeMap::new(),
						processes: BTreeMap::new(),
						closed: false,
					}),
				})
			})
			.clone()
	}

	/// Stops every stdio session of the Run and saves the data of each plugin it restored. A
	/// later use starts them again. While an MCP process of the Run may still run, its data is
	/// neither saved nor removed and the state stays, so the next close or start tries again.
	pub async fn close(&self, run: Uuid) {
		let Some(entry) = self
			.runs
			.lock()
			.expect("stdio sessions lock")
			.get(&run)
			.cloned()
		else {
			return;
		};
		// The Run stays in the map until this close has finished with the Pod, so a concurrent
		// call waits for it instead of starting a session whose data this close then removes.
		let _exclusive = entry.activity.write().await;
		let mut state = entry.state.lock().await;
		if state.closed {
			return;
		}
		if let Some(pod) = state.pod.clone()
			&& !close_sessions(&mut state, &pod, run).await
		{
			return;
		}
		state.closed = true;
		let mut runs = self.runs.lock().expect("stdio sessions lock");
		if runs
			.get(&run)
			.is_some_and(|current| Arc::ptr_eq(current, &entry))
		{
			runs.remove(&run);
		}
	}
}

impl RunEntry {
	async fn forget(&self, operation_id: &str) {
		self.state.lock().await.processes.remove(operation_id);
	}

	async fn recover(&self, run: Uuid) -> bool {
		let _exclusive = self.activity.write().await;
		let mut state = self.state.lock().await;
		if let Some(pod) = state.pod.clone() {
			settle(&mut state, &pod, run).await;
		}
		!state.has_abandoned()
	}
}

/// Stops the Run's sessions and accounts for every MCP process it may have started, then saves
/// and removes its plugin data. False when a process is still unaccounted for: nothing is saved
/// or removed then.
async fn close_sessions(state: &mut RunSessions, pod: &StdioPod, run: Uuid) -> bool {
	for operation_id in state.detach_sessions() {
		if stop_session(pod, run, &operation_id).await {
			state.processes.remove(&operation_id);
		}
	}
	// The Runner's own records reach a process whose id this server never kept.
	if let Err(error) = pod
		.runtime
		.mcp_stop_run(&pod.settings.scratch_space_id, &run.to_string())
		.await
	{
		tracing::info!(%run, %error, "the Runner did not stop the Run's stdio sessions");
	}
	settle(state, pod, run).await;
	if !state.processes.is_empty() {
		tracing::warn!(%run, pending = state.processes.len(), "stdio MCP processes not accounted for; plugin data kept in place");
		return false;
	}
	// Only plugins whose sessions passed the gate restored data; the host checks the gate
	// again when it saves, so a Run that lost its lease or authority keeps the previous
	// version.
	for plugin in std::mem::take(&mut state.restored) {
		if let Err(cause) = save_data(&state.host, pod, run, &plugin).await {
			tracing::warn!(%run, plugin = plugin.as_str(), cause, "plugin data not saved");
			state.host.data_not_saved(&plugin, &cause).await;
		}
		// The next start restores the saved version, so the live area goes either way.
		let live = stdio_session::plugin_data(run, &plugin);
		if command(pod, run, format!("rm -rf -- {}", quote(&live)))
			.await
			.is_err()
		{
			tracing::info!(%run, plugin = plugin.as_str(), "live plugin data not removed");
		}
	}
	// Each Run has its own plugin trees, so another Run's sessions never lose theirs.
	let trees = stdio_session::run_plugins(run);
	if command(pod, run, format!("rm -rf -- {}", quote(&trees)))
		.await
		.is_err()
	{
		tracing::info!(%run, "plugin trees not removed");
	}
	true
}

/// Accounts for abandoned processes under the exclusive activity guard. A Run-wide stop may
/// end cached siblings too, so detach those sessions before sending it.
async fn settle(state: &mut RunSessions, pod: &StdioPod, run: Uuid) {
	let abandoned: Vec<_> = state
		.processes
		.iter()
		.filter(|(_, process)| process.abandoned())
		.map(|(id, _)| id.clone())
		.collect();
	for operation_id in abandoned {
		match account(pod, run, &operation_id).await {
			Accounting::Confirmed => {
				state.processes.remove(&operation_id);
			}
			Accounting::NeedsRunStop => {
				let cached = state.detach_sessions();
				match pod
					.runtime
					.mcp_stop_run(&pod.settings.scratch_space_id, &run.to_string())
					.await
				{
					Ok(()) => {
						state.processes.remove(&operation_id);
						for id in cached {
							state.processes.remove(&id);
						}
					}
					Err(error) => {
						tracing::info!(%run, operation_id, %error, "stdio MCP Run stop failed");
					}
				}
			}
			Accounting::Unconfirmed => {}
		}
	}
}

enum Accounting {
	Confirmed,
	NeedsRunStop,
	Unconfirmed,
}

/// Fence a delayed start, or stop the process by id. Only exclusive recovery may fall back
/// to stopping every process in the Run.
async fn account(pod: &StdioPod, run: Uuid, operation_id: &str) -> Accounting {
	match pod
		.runtime
		.refuse_command(
			operation_id,
			&pod.settings.scratch_space_id,
			&run.to_string(),
		)
		.await
	{
		Ok(CommandRefusal { refused: true, .. }) => Accounting::Confirmed,
		Ok(_) if stop_session(pod, run, operation_id).await => Accounting::Confirmed,
		// A stop by id cannot confirm a session whose Pod was replaced. The Runner's Run
		// stop can check that the Pod is gone or contains no process for the operation.
		Ok(_) => Accounting::NeedsRunStop,
		Err(error) => {
			tracing::info!(%run, operation_id, %error, "stdio MCP start not fenced");
			Accounting::Unconfirmed
		}
	}
}

struct RunStdio {
	sessions: Arc<StdioSessions>,
	run: Uuid,
	host: Arc<dyn StdioPluginHost>,
}

struct OpenSession {
	entry: Arc<RunEntry>,
	pod: StdioPod,
	slot: OwnedMutexGuard<Option<Session>>,
	_activity: OwnedRwLockReadGuard<()>,
}

impl OpenSession {
	async fn retire(&mut self, run: Uuid) {
		let session = self.slot.take().expect("session was open");
		if stop_session(&self.pod, run, &session.operation_id).await {
			self.entry.forget(&session.operation_id).await;
		}
	}
}

impl RunStdio {
	fn check(&self, run: Uuid) -> Result<()> {
		if run == self.run {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}

	async fn open(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &StdioServer,
		admitted: bool,
	) -> std::result::Result<OpenSession, Setup> {
		let key = key(server);
		loop {
			let entry = self.sessions.run(run, &self.host);
			let activity = entry.activity.clone().read_owned().await;
			let slot = {
				let mut state = entry.state.lock().await;
				if state.closed {
					continue;
				}
				state.sessions.entry(key.clone()).or_default().clone()
			};
			let mut slot = slot.lock_owned().await;
			if slot.is_none() {
				self.admit_start(admitted).await?;
			}
			let mut state = entry.state.lock().await;
			state.host = self.host.clone();
			let (pod, startup) = if slot.is_some() {
				(
					state
						.pod
						.clone()
						.ok_or_else(|| Setup::new(STDIO_UNAVAILABLE))?,
					None,
				)
			} else {
				if state.has_abandoned() {
					drop(state);
					drop(slot);
					drop(activity);
					if !entry.recover(run).await {
						return Err(Setup::new(START_FAILED));
					}
					continue;
				}
				let (pod, request, session) =
					prepare_start(&mut state, run, server, config).await?;
				(pod, Some((request, session)))
			};
			drop(state);
			if let Some((request, session)) = startup {
				*slot = Some(start(&entry, &pod, run, request, session).await?);
			}
			return Ok(OpenSession {
				entry,
				pod,
				slot,
				_activity: activity,
			});
		}
	}

	/// A call already passed the principal and usage checks. A listing must check them too
	/// before starting a process.
	async fn admit_start(&self, admitted: bool) -> std::result::Result<(), Setup> {
		if !self.host.approved().await? {
			return Err(Setup::new(NOT_APPROVED));
		}
		if !admitted {
			match self.host.admit_call().await {
				Ok(()) => {}
				Err(Error::Forbidden | Error::Unauthorized | Error::AuthorityRefused(_)) => {
					return Err(Setup::new(REFUSED));
				}
				Err(Error::Refused(code)) => return Err(Setup(code.to_ascii_lowercase())),
				Err(_) => return Err(Setup::new(START_FAILED)),
			}
		}
		Ok(())
	}
}

#[async_trait]
impl StdioPluginSessions for RunStdio {
	async fn list(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &StdioServer,
	) -> Result<Vec<ListedTool>> {
		self.check(run)?;
		let opened = self
			.open(run, server, config, false)
			.await
			.map_err(|Setup(cause)| Error::Invalid(cause))?;
		Ok(opened
			.slot
			.as_ref()
			.expect("session was open")
			.listed
			.clone())
	}

	async fn call(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &StdioServer,
		tool: &str,
		schema_digest: &str,
		arguments: Value,
		_key: &str,
	) -> Result<Value> {
		self.check(run)?;
		if !arguments.is_object() {
			return Err(Error::Invalid("tool arguments must be an object".into()));
		}
		// Approval is read again at every call, so a withdrawn approval stops a running session's
		// use at once.
		if !self.host.approved().await? {
			return Err(Error::Invalid(format!(
				"MCP_STDIO_NOT_APPROVED: the Tenant does not approve the mcp_stdio group ({NOT_APPROVED})"
			)));
		}
		self.host.admit_call().await?;
		let mut opened = self
			.open(run, server, config, true)
			.await
			.map_err(|Setup(cause)| not_delivered(&cause))?;
		let session = opened.slot.as_mut().expect("session was open");
		// Every call checks the tool the Run pinned (ADR 0073), not only the call that started
		// the session: a kept session must not run a tool whose schema changed.
		if session.digests.get(tool).map(String::as_str) != Some(schema_digest) {
			return Err(Error::Invalid(format!(
				"CAPABILITY_CHANGED: the server no longer offers {tool} as pinned"
			)));
		}
		let message = session.request("tools/call", json!({"name": tool, "arguments": arguments}));
		let reply = opened
			.pod
			.runtime
			.mcp_call(&McpCallRequest {
				operation_id: session.operation_id.clone(),
				message,
				timeout_seconds: CALL_SECONDS,
			})
			.await;
		match reply {
			Ok(reply) => call_result(reply.message),
			Err(ExecutionPodError::Remote { code, message }) if unsent(&code) => {
				// The session is gone or busy, so the request never reached the server. A busy
				// one still runs, so it is stopped before it is forgotten.
				opened.retire(run).await;
				Err(Error::ToolNotDelivered {
					message: format!("{code}: {message}"),
					retry_after: None,
				})
			}
			Err(ExecutionPodError::Remote {
				code: ExecutionPodErrorCode::InvalidRequest,
				message,
			}) => Err(Error::Invalid(format!("MCP_REQUEST_INVALID: {message}"))),
			Err(error) => {
				// The helper ends a session whose call failed; the request may have reached the
				// server, so it is reconciled and never sent again. The stop makes sure it ended.
				opened.retire(run).await;
				tracing::warn!(%run, %error, "stdio MCP call outcome unknown");
				match error {
					ExecutionPodError::Transport(Error::Port(source))
						if source
							.is::<crate::ports::capabilities::execution_pod::RunnerTimeout>() =>
					{
						Err(Error::Port(source))
					}
					_ => Err(Error::External("MCP answer was lost after send".into())),
				}
			}
		}
	}
}

/// Stops a session that is being dropped. False when the stop was not confirmed: the caller
/// keeps its id pending, so the Run's close or next start stops it.
async fn stop_session(pod: &StdioPod, run: Uuid, operation_id: &str) -> bool {
	match pod.runtime.mcp_stop(operation_id).await {
		Ok(()) => true,
		Err(error) => {
			tracing::info!(%run, operation_id, %error, "stdio session stop failed");
			false
		}
	}
}

fn key(server: &PluginServer) -> SessionKey {
	(
		server.plugin.clone(),
		server.digest.clone(),
		server.server.clone(),
	)
}

fn not_delivered(cause: &str) -> Error {
	Error::ToolNotDelivered {
		message: format!("stdio MCP session unavailable: {cause}"),
		retry_after: None,
	}
}

/// Runner codes that prove a call never reached the server.
fn unsent(code: &ExecutionPodErrorCode) -> bool {
	matches!(code, ExecutionPodErrorCode::Unknown(code) if code == "mcp_unavailable" || code == "mcp_busy")
}

/// A JSON-RPC answer to `tools/call` as [`PluginSessions`](crate::ports::tools::PluginSessions)
/// returns it.
fn call_result(message: Value) -> Result<Value> {
	if let Some(result) = message.get("result") {
		if result.get("isError") == Some(&Value::Bool(true)) {
			return Ok(
				json!({"is_error": true, "content": result.get("content").cloned().unwrap_or(json!([]))}),
			);
		}
		return Ok(result.clone());
	}
	if let Some(error) = message.get("error") {
		let text = error
			.get("message")
			.and_then(Value::as_str)
			.unwrap_or("MCP error");
		let end = text
			.char_indices()
			.map(|(index, c)| index + c.len_utf8())
			.take_while(|end| *end <= MAX_ERROR_MESSAGE)
			.last()
			.unwrap_or(0);
		return Ok(json!({"error": {"kind": "mcp_error", "message": &text[..end]}}));
	}
	// The server answered something that is not a response; whether the call ran is unknown.
	Err(Error::External("MCP answer was unreadable".into()))
}

/// Prepares the shared Pod and plugin files while holding the Run state lock. Register the
/// operation before releasing the lock, so a sibling keeps this tree even during startup.
async fn prepare_start(
	state: &mut RunSessions,
	run: Uuid,
	server: &PluginServer,
	config: &StdioServer,
) -> std::result::Result<(StdioPod, McpStartRequest, Session), Setup> {
	let launch =
		stdio_session::launch(&server.digest, &server.plugin, run, config).map_err(Setup::new)?;
	let pod = match &state.pod {
		Some(pod) => pod.clone(),
		None => {
			let pod = state
				.host
				.pod()
				.await?
				.ok_or_else(|| Setup::new(STDIO_UNAVAILABLE))?;
			state.pod = Some(pod.clone());
			pod
		}
	};
	pod.runtime.ensure(&pod.settings).await?;
	let restore = !state.restored.contains(&server.plugin);
	let tree = state.host.plugin_files(&server.digest).await?;
	// Without a saved version the live area stays as it is: a Run's area starts empty, and one
	// that paused keeps what its servers wrote.
	let data = if restore {
		state.host.load_data(&server.plugin).await?
	} else {
		None
	};
	let write_tree = !state
		.processes
		.values()
		.any(|process| process.plugin_root == launch.plugin_root);
	prepare(&pod, run, &launch, &tree, data.as_deref(), write_tree).await?;
	if restore {
		state.restored.insert(server.plugin.clone());
	}
	let owner = Arc::new(());
	let process = Process {
		plugin_root: launch.plugin_root,
		owner: Arc::downgrade(&owner),
	};
	let session = Session {
		operation_id: Uuid::new_v4().to_string(),
		_process_owner: owner,
		digests: BTreeMap::new(),
		listed: Vec::new(),
		next_id: 0,
	};
	state
		.processes
		.insert(session.operation_id.clone(), process);
	let request = McpStartRequest {
		operation_id: session.operation_id.clone(),
		scratch_space_id: pod.settings.scratch_space_id.clone(),
		run_id: run.to_string(),
		// Lockfiles pinned at attach time would install here; until then a package fetcher
		// is refused by `launch`.
		install: String::new(),
		command: launch.command,
		env: launch.env,
		plugin_data: stdio_session::in_pod(&launch.plugin_data),
		proxy_credential: pod.credential.clone(),
	};
	Ok((pod, request, session))
}

/// Start and list one server without holding the Run state lock.
async fn start(
	entry: &RunEntry,
	pod: &StdioPod,
	run: Uuid,
	request: McpStartRequest,
	mut session: Session,
) -> std::result::Result<Session, Setup> {
	if let Err(error) = pod.runtime.mcp_start(&request).await {
		if matches!(
			account(pod, run, &session.operation_id).await,
			Accounting::Confirmed
		) {
			entry.forget(&session.operation_id).await;
		}
		return Err(error.into());
	}
	if let Err(error) = handshake(pod, &mut session).await {
		if stop_session(pod, run, &session.operation_id).await {
			entry.forget(&session.operation_id).await;
		}
		return Err(error);
	}
	Ok(session)
}

async fn handshake(pod: &StdioPod, session: &mut Session) -> std::result::Result<(), Setup> {
	let initialize = session.request(
		"initialize",
		json!({
			"protocolVersion": PROTOCOL_VERSION,
			"capabilities": {},
			"clientInfo": {"name": "aidash", "version": env!("CARGO_PKG_VERSION")},
		}),
	);
	let answer = exchange(pod, session, initialize).await?;
	if answer
		.get("result")
		.is_none_or(|result| !result.is_object())
	{
		return Err(Setup::new(INITIALIZE_FAILED));
	}
	exchange(
		pod,
		session,
		json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
	)
	.await?;
	let operation_id = session.operation_id.clone();
	let tools = mcp_listing::list(|cursor| {
		let params = cursor.map_or_else(|| json!({}), |cursor| json!({"cursor": cursor}));
		let message = session.request("tools/list", params);
		let request = McpCallRequest {
			operation_id: operation_id.clone(),
			message,
			timeout_seconds: mcp_listing::MCP_LIST_TIMEOUT.as_secs() as u32,
		};
		async move {
			let mut answer = pod.runtime.mcp_call(&request).await?.message;
			let page = answer
				.get_mut("result")
				.ok_or_else(|| Setup::new(INVALID_RESPONSE))?;
			serde_json::from_value::<mcp_listing::Page<Value>>(page.take())
				.map_err(|_| Setup::new(INVALID_RESPONSE))
		}
	})
	.await?;
	for tool in tools {
		let listed = ListedTool {
			name: tool
				.get("name")
				.and_then(Value::as_str)
				.ok_or_else(|| Setup::new(INVALID_RESPONSE))?
				.to_owned(),
			description: tool
				.get("description")
				.and_then(Value::as_str)
				.map(str::to_owned),
			input_schema: tool.get("inputSchema").cloned().unwrap_or(Value::Null),
		};
		session.digests.insert(
			listed.name.clone(),
			aidash_domain::agent_plugin::schema_digest(&listed.input_schema),
		);
		session.listed.push(listed);
	}
	Ok(())
}

async fn exchange(
	pod: &StdioPod,
	session: &Session,
	message: Value,
) -> std::result::Result<Value, Setup> {
	Ok(pod
		.runtime
		.mcp_call(&McpCallRequest {
			operation_id: session.operation_id.clone(),
			message,
			timeout_seconds: SETUP_SECONDS,
		})
		.await?
		.message)
}

/// A path as one shell word.
fn quote(value: &str) -> String {
	format!("'{}'", value.replace('\'', r"'\''"))
}

/// Every directory that holds one of `files` below `base`, and `base` itself.
fn directories<'a>(base: &str, files: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
	let mut directories = BTreeSet::from([base.to_owned()]);
	for path in files {
		if let Some((parent, _)) = path.rsplit_once('/') {
			directories.insert(format!("{base}/{parent}"));
		}
	}
	directories
}

/// Writes the plugin tree afresh, since anything in the Pod could have changed it, and, on a
/// plugin's first session in the Run, its saved data in place of the live area.
async fn prepare(
	pod: &StdioPod,
	run: Uuid,
	launch: &Launch,
	tree: &[PluginFile],
	data: Option<&[PluginFile]>,
	write_tree: bool,
) -> std::result::Result<(), Setup> {
	// A tree that a live session of this Run already runs from is kept: deleting it would
	// leave that session in a removed working directory.
	let mut removed = vec![];
	let mut made = BTreeSet::new();
	if write_tree {
		removed.push(quote(&launch.plugin_root));
		made = directories(
			&launch.plugin_root,
			tree.iter().map(|file| file.path.as_str()),
		);
	}
	if let Some(data) = data {
		removed.push(quote(&launch.plugin_data));
		made.extend(directories(
			&launch.plugin_data,
			data.iter().map(|file| file.path.as_str()),
		));
	}
	if !made.is_empty() {
		let mut script = format!("rm -rf -- {} && mkdir -p --", removed.join(" "));
		for directory in &made {
			script.push(' ');
			script.push_str(&quote(directory));
		}
		command(pod, run, script).await?;
	}
	for file in tree {
		let executable = launch.executable.as_deref() == Some(file.path.as_str());
		// On a kept tree only this server's executable is written again, to make it runnable.
		if !write_tree && !executable {
			continue;
		}
		let mode = if executable { 0o755 } else { 0o644 };
		let path = format!("{}/{}", launch.plugin_root, file.path);
		write(pod, run, &path, &file.bytes, mode).await?;
	}
	for file in data.unwrap_or_default() {
		let path = format!("{}/{}", launch.plugin_data, file.path);
		write(pod, run, &path, &file.bytes, 0o600).await?;
	}
	Ok(())
}

/// Runs a short preparation command in the Pod and waits for it to succeed.
async fn command(pod: &StdioPod, run: Uuid, code: String) -> std::result::Result<(), Setup> {
	let operation_id = Uuid::new_v4().to_string();
	let mut result = pod
		.runtime
		.execute(&CommandRequest {
			operation_id: operation_id.clone(),
			scratch_space_id: pod.settings.scratch_space_id.clone(),
			run_id: run.to_string(),
			code,
			expected_session_id: None,
			cwd: None,
			timeout_seconds: Some(PREPARE_SECONDS),
			background: false,
			notify_on_exit: false,
			proxy_credential: None,
		})
		.await?;
	let deadline =
		tokio::time::Instant::now() + Duration::from_secs(u64::from(PREPARE_SECONDS) + 30);
	while matches!(result.status.as_str(), "queued" | "running") {
		if tokio::time::Instant::now() >= deadline {
			return Err(Setup::new(START_FAILED));
		}
		tokio::time::sleep(PREPARE_POLL).await;
		result = pod.runtime.result(&operation_id).await?;
	}
	if result.exit_code == Some(0) {
		Ok(())
	} else {
		Err(Setup::new(START_FAILED))
	}
}

async fn write(
	pod: &StdioPod,
	run: Uuid,
	path: &str,
	bytes: &[u8],
	mode: u32,
) -> std::result::Result<(), Setup> {
	// An empty file is one empty final chunk.
	let mut chunks = bytes.chunks(CHUNK).peekable();
	let mut offset = 0u64;
	loop {
		let chunk = chunks.next().unwrap_or_default();
		let last = chunks.peek().is_none();
		pod.runtime
			.file_write(&FileWriteRequest {
				scratch_space_id: pod.settings.scratch_space_id.clone(),
				run_id: run.to_string(),
				path: path.to_owned(),
				data: STANDARD.encode(chunk),
				offset,
				final_chunk: last,
				mode: Some(mode),
			})
			.await?;
		offset += chunk.len() as u64;
		if last {
			return Ok(());
		}
	}
}

/// Lists, checks, reads and replaces one plugin's saved data. Any failure keeps the previous
/// version; the cause is returned.
async fn save_data(
	host: &Arc<dyn StdioPluginHost>,
	pod: &StdioPod,
	run: Uuid,
	plugin: &PluginName,
) -> std::result::Result<(), String> {
	let root = stdio_session::plugin_data(run, plugin);
	let manifest = pod
		.runtime
		.file_manifest(&FileManifestRequest {
			scratch_space_id: pod.settings.scratch_space_id.clone(),
			root: root.clone(),
			ignore_files: Vec::new(),
			exclude: Vec::new(),
			// `.git` is listed only when included.
			include: vec!["**".into()],
			max_files: None,
			missing_ok: false,
		})
		.await
		.map_err(|error| Setup::from(error).0)?;
	let prefix = format!("{root}/");
	let relative: Vec<(&str, u64, &str)> = manifest
		.files
		.iter()
		.map(|file| {
			file.path
				.strip_prefix(&prefix)
				.map(|path| (path, file.size, file.sha256.as_str()))
				.ok_or_else(|| stdio_session::DATA_INVALID_PATH.to_owned())
		})
		.collect::<std::result::Result<_, _>>()?;
	stdio_session::check_data(
		relative.iter().map(|(path, size, _)| (*path, *size)),
		manifest.truncated,
	)
	.map_err(str::to_owned)?;
	let mut files = Vec::with_capacity(relative.len());
	for (path, size, sha256) in relative {
		let bytes = read(pod, &format!("{prefix}{path}"), size).await?;
		let digest = hex(&Sha256::digest(&bytes));
		if bytes.len() as u64 != size || digest != sha256.trim_start_matches("sha256:") {
			return Err("plugin_data_changed_while_saving".into());
		}
		files.push(PluginFile {
			path: path.to_owned(),
			bytes,
		});
	}
	host.save_data(plugin, files)
		.await
		.map_err(|error| format!("plugin_data_save_failed: {error}"))
}

async fn read(pod: &StdioPod, path: &str, size: u64) -> std::result::Result<Vec<u8>, String> {
	let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
	let mut offset = 0u64;
	loop {
		let chunk = pod
			.runtime
			.file_read(&FileReadRequest {
				scratch_space_id: pod.settings.scratch_space_id.clone(),
				path: path.to_owned(),
				offset: Some(offset),
				limit: CHUNK as u32,
			})
			.await
			.map_err(|error| Setup::from(error).0)?;
		bytes.extend(
			STANDARD
				.decode(chunk.data.as_bytes())
				.map_err(|_| INVALID_RESPONSE.to_owned())?,
		);
		if bytes.len() as u64 > size {
			return Err("plugin_data_changed_while_saving".into());
		}
		if chunk.eof || chunk.next_offset <= offset {
			return Ok(bytes);
		}
		offset = chunk.next_offset;
	}
}

fn hex(bytes: &[u8]) -> String {
	bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
