//! Sessions with the stdio MCP servers of Agent Plugins (ADR 0073 and its 2026-11-14
//! amendment). Each runs as a process in the Execution Pod of its Run's Scratch Space with the
//! Run credential of ADR 0136: one session per Run and server, started on first use, stopped
//! when the Run's sessions close. `${PLUGIN_DATA}` is restored before a plugin's first session in
//! the Run and saved, after a check, when they close.
use crate::{
	Error, Result,
	ports::{
		capabilities::execution_pod::{
			CommandRequest, ExecutionPodError, ExecutionPodErrorCode, FileManifestRequest,
			FileReadRequest, FileWriteRequest, McpCallRequest, McpStartRequest,
		},
		tools::{PluginServer, StdioPluginHost, StdioPluginSessions, StdioPod},
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
	sync::{Arc, Mutex},
	time::Duration,
};
use uuid::Uuid;

/// How long one tool call may take in the server.
pub const CALL_SECONDS: u32 = 600;
/// How long the handshake and each `tools/list` page may take.
const SETUP_SECONDS: u32 = 60;
const MAX_LIST_PAGES: usize = 16;
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

/// The live sessions of every Run in this process, shared by every clone of the Store that
/// holds them.
#[derive(Default)]
pub struct StdioSessions {
	runs: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<RunSessions>>>>,
}

struct RunSessions {
	host: Arc<dyn StdioPluginHost>,
	pod: Option<StdioPod>,
	/// Plugins whose saved data this Run restored, so their data is saved at close.
	restored: BTreeSet<PluginName>,
	sessions: BTreeMap<SessionKey, Session>,
}

struct Session {
	operation_id: String,
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
impl From<ExecutionPodError> for Setup {
	fn from(error: ExecutionPodError) -> Self {
		match error {
			ExecutionPodError::Remote { code, .. } => Self(code.to_string()),
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

	fn run(
		&self,
		run: Uuid,
		host: &Arc<dyn StdioPluginHost>,
	) -> Arc<tokio::sync::Mutex<RunSessions>> {
		self.runs
			.lock()
			.expect("stdio sessions lock")
			.entry(run)
			.or_insert_with(|| {
				Arc::new(tokio::sync::Mutex::new(RunSessions {
					host: host.clone(),
					pod: None,
					restored: BTreeSet::new(),
					sessions: BTreeMap::new(),
				}))
			})
			.clone()
	}

	/// Stops every stdio session of the Run and saves the data of each plugin it restored. A
	/// later use starts them again.
	pub async fn close(&self, run: Uuid) {
		let Some(state) = self.runs.lock().expect("stdio sessions lock").remove(&run) else {
			return;
		};
		let mut state = state.lock().await;
		let Some(pod) = state.pod.clone() else {
			return;
		};
		for (_, session) in std::mem::take(&mut state.sessions) {
			if let Err(error) = pod.runtime.mcp_stop(&session.operation_id).await {
				tracing::info!(%run, operation_id = session.operation_id, %error, "stdio session stop failed");
			}
		}
		// Only plugins whose sessions passed the gate restored data; the host checks the gate
		// again when it saves, so a Run that lost its lease or authority keeps the previous
		// version.
		for plugin in std::mem::take(&mut state.restored) {
			if let Err(cause) = save_data(&state.host, &pod, run, &plugin).await {
				tracing::warn!(%run, plugin = plugin.as_str(), cause, "plugin data not saved");
				state.host.data_not_saved(&plugin, &cause).await;
			}
			// The next start restores the saved version, so the live area goes either way.
			let live = stdio_session::plugin_data(run, &plugin);
			if command(&pod, run, format!("rm -rf -- {}", quote(&live)))
				.await
				.is_err()
			{
				tracing::info!(%run, plugin = plugin.as_str(), "live plugin data not removed");
			}
		}
	}
}

struct RunStdio {
	sessions: Arc<StdioSessions>,
	run: Uuid,
	host: Arc<dyn StdioPluginHost>,
}

impl RunStdio {
	fn check(&self, run: Uuid) -> Result<()> {
		if run == self.run {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
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
		let state = self.sessions.run(run, &self.host);
		let mut state = state.lock().await;
		state.host = self.host.clone();
		let key = key(server);
		if let Some(session) = state.sessions.get(&key) {
			return Ok(session.listed.clone());
		}
		let session = start(&mut state, run, server, config, false)
			.await
			.map_err(|Setup(cause)| Error::Invalid(cause))?;
		let listed = session.listed.clone();
		state.sessions.insert(key, session);
		Ok(listed)
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
		let state = self.sessions.run(run, &self.host);
		let mut state = state.lock().await;
		state.host = self.host.clone();
		let key = key(server);
		if !state.sessions.contains_key(&key) {
			let session = start(&mut state, run, server, config, true)
				.await
				.map_err(|Setup(cause)| not_delivered(&cause))?;
			// A session started for this call checks the tool the Run pinned (ADR 0073).
			if session.digests.get(tool).map(String::as_str) != Some(schema_digest) {
				state.sessions.insert(key, session);
				return Err(Error::Invalid(format!(
					"CAPABILITY_CHANGED: the server no longer offers {tool} as pinned"
				)));
			}
			state.sessions.insert(key.clone(), session);
		}
		let pod = state
			.pod
			.clone()
			.ok_or_else(|| not_delivered(STDIO_UNAVAILABLE))?;
		let session = state.sessions.get_mut(&key).expect("session started above");
		let message = session.request("tools/call", json!({"name": tool, "arguments": arguments}));
		let reply = pod
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
				// The session is gone or busy, so the request never reached the server.
				state.sessions.remove(&key);
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
				// server, so it is reconciled and never sent again.
				state.sessions.remove(&key);
				tracing::warn!(%run, %error, "stdio MCP call outcome unknown");
				Err(Error::External("MCP answer was lost after send".into()))
			}
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

/// Starts the server's session: Pod, plugin tree, restored data, process, handshake, listing.
/// `admitted` says the caller already passed [`StdioPluginHost::admit_call`] for this use.
async fn start(
	state: &mut RunSessions,
	run: Uuid,
	server: &PluginServer,
	config: &StdioServer,
	admitted: bool,
) -> std::result::Result<Session, Setup> {
	if !state.host.approved().await? {
		return Err(Setup::new(NOT_APPROVED));
	}
	// No process starts unless the Run passes the checks a call passes.
	if !admitted {
		match state.host.admit_call().await {
			Ok(()) => {}
			Err(Error::Forbidden | Error::Unauthorized | Error::AuthorityRefused(_)) => {
				return Err(Setup::new(REFUSED));
			}
			Err(Error::Refused(code)) => return Err(Setup(code.to_ascii_lowercase())),
			Err(_) => return Err(Setup::new(START_FAILED)),
		}
	}
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
	prepare(&pod, run, &launch, &tree, data.as_deref()).await?;
	if restore {
		state.restored.insert(server.plugin.clone());
	}
	let operation_id = Uuid::new_v4().to_string();
	pod.runtime
		.mcp_start(&McpStartRequest {
			operation_id: operation_id.clone(),
			scratch_space_id: pod.settings.scratch_space_id.clone(),
			run_id: run.to_string(),
			// Lockfiles pinned at attach time would install here; until then a package fetcher
			// is refused by `launch`.
			install: String::new(),
			command: launch.command.clone(),
			env: launch.env.clone(),
			plugin_data: stdio_session::in_pod(&launch.plugin_data),
			proxy_credential: pod.credential.clone(),
		})
		.await?;
	let mut session = Session {
		operation_id,
		digests: BTreeMap::new(),
		listed: Vec::new(),
		next_id: 0,
	};
	match handshake(&pod, &mut session).await {
		Ok(()) => Ok(session),
		Err(error) => {
			let _ = pod.runtime.mcp_stop(&session.operation_id).await;
			Err(error)
		}
	}
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
	let mut cursor: Option<String> = None;
	for _ in 0..MAX_LIST_PAGES {
		let params = match &cursor {
			Some(cursor) => json!({"cursor": cursor}),
			None => json!({}),
		};
		let list = session.request("tools/list", params);
		let answer = exchange(pod, session, list).await?;
		let page = answer
			.get("result")
			.ok_or_else(|| Setup::new(INVALID_RESPONSE))?;
		for tool in page
			.get("tools")
			.and_then(Value::as_array)
			.ok_or_else(|| Setup::new(INVALID_RESPONSE))?
		{
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
		cursor = page
			.get("nextCursor")
			.and_then(Value::as_str)
			.map(str::to_owned);
		if cursor.is_none() {
			return Ok(());
		}
	}
	Err(Setup::new(INVALID_RESPONSE))
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
) -> std::result::Result<(), Setup> {
	let mut script = format!("rm -rf -- {root}", root = quote(&launch.plugin_root));
	if data.is_some() {
		script.push_str(&format!(" {}", quote(&launch.plugin_data)));
	}
	let mut made = directories(
		&launch.plugin_root,
		tree.iter().map(|file| file.path.as_str()),
	);
	if let Some(data) = data {
		made.extend(directories(
			&launch.plugin_data,
			data.iter().map(|file| file.path.as_str()),
		));
	}
	script.push_str(" && mkdir -p --");
	for directory in &made {
		script.push(' ');
		script.push_str(&quote(directory));
	}
	command(pod, run, script).await?;
	for file in tree {
		let mode = if launch.executable.as_deref() == Some(file.path.as_str()) {
			0o755
		} else {
			0o644
		};
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
			cwd: None,
			timeout_seconds: Some(PREPARE_SECONDS),
			background: false,
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
			max_files: stdio_session::MAX_PLUGIN_DATA_FILES,
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
