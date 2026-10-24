//! A Run's MCP sessions with the remote servers of its Agent Plugins (ADR 0073). A session
//! outlives the harness step that opened it, so the sessions live in one process-wide map that
//! opens a missing entry on demand and gives up idle ones.
use crate::{
	Error, Result,
	tools::{TOOL_TIMEOUT, ToolClients},
};
use aidash_application::ports::tools::PluginInputs;
use aidash_application::{
	ports::tools::{ListFailure, PluginServer, PluginSessions},
	provider_access::KeylessAccess,
};
use aidash_domain::agent_plugin::interaction::{
	CallCheckpoint, InputRequest, RequiredInput, ToolRound,
};
use aidash_domain::agent_plugin::{
	ListedTool, PluginName, RemoteServer, ServerName, schema_digest,
};
use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
#[allow(deprecated)] // Both accepted MCP revisions still require sampling.
use rmcp::{
	ClientHandler, ErrorData,
	model::{
		CallToolResponse, ClientConfig, CreateMessageRequestParams, CreateMessageResult,
		ElicitRequestParams, ElicitResult,
	},
	service::RequestContext,
};
use rmcp::{
	ClientLifecycleMode, ClientServiceExt, RoleClient,
	model::{CallToolRequestParams, PaginatedRequestParams},
	service::{ClientInitializeError, RunningService, ServiceError},
	transport::{
		StreamableHttpClientTransport,
		streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
	},
};
use rmcp_modern as rmcp;
use serde_json::Value;
use std::{
	collections::HashMap,
	sync::{Arc, Mutex, MutexGuard, PoisonError},
	time::{Duration, Instant},
};
use tokio::time::timeout;
use uuid::Uuid;

#[derive(Default)]
struct ActiveCall {
	open: bool,
	input: Option<Arc<dyn PluginInputs>>,
}
#[derive(Default)]
struct Inputs {
	sequence: std::sync::atomic::AtomicU64,
	active: Arc<Mutex<ActiveCall>>,
	serial: tokio::sync::Mutex<()>,
}
struct InputScope(Arc<Mutex<ActiveCall>>);
impl Drop for InputScope {
	fn drop(&mut self) {
		*self.0.lock().unwrap_or_else(PoisonError::into_inner) = ActiveCall::default();
	}
}
impl Inputs {
	fn enter(&self, input: Option<Arc<dyn PluginInputs>>) -> Result<InputScope> {
		let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
		if active.open {
			return Err(Error::Conflict("MCP_CALL_OVERLAP".into()));
		}
		*active = ActiveCall { open: true, input };
		Ok(InputScope(self.active.clone()))
	}
	async fn request(
		&self,
		id: String,
		method: &str,
		params: Value,
	) -> std::result::Result<Value, ErrorData> {
		let _serial = self
			.serial
			.try_lock()
			.map_err(|_| ErrorData::invalid_request("MCP_INPUT_OVERLAP", None))?;
		let input = self
			.active
			.lock()
			.unwrap_or_else(PoisonError::into_inner)
			.input
			.clone()
			.ok_or_else(|| ErrorData::invalid_request("MCP_INPUT_DENIED", None))?;
		input
			.request(
				format!(
					"{}:{id}",
					self.sequence
						.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
				),
				InputRequest {
					method: method.into(),
					params,
				},
			)
			.await
			.map_err(|_| ErrorData::internal_error("MCP input was refused or failed", None))
	}
}
#[allow(deprecated)] // Sampling remains part of Aidash's client contract.
impl ClientHandler for Inputs {
	async fn list_roots(
		&self,
		_context: RequestContext<RoleClient>,
	) -> std::result::Result<rmcp::model::ListRootsResult, ErrorData> {
		Err(ErrorData::method_not_found::<
			rmcp::model::ListRootsRequestMethod,
		>())
	}
	fn get_info(&self) -> ClientConfig {
		serde_json::from_value(serde_json::json!({
			"protocolVersion":"2026-07-28", "capabilities":{"sampling":{},"elicitation":{"form":{}}},
			"clientInfo":{"name":"aidash","version":env!("CARGO_PKG_VERSION")}
		}))
		.expect("static MCP client configuration")
	}
	async fn create_message(
		&self,
		params: CreateMessageRequestParams,
		context: RequestContext<RoleClient>,
	) -> std::result::Result<CreateMessageResult, ErrorData> {
		let raw = serde_json::to_value(params)
			.map_err(|_| ErrorData::invalid_params("MCP_SAMPLING_INVALID", None))?;
		let value = self
			.request(context.id.to_string(), "sampling/createMessage", raw)
			.await?;
		serde_json::from_value(value)
			.map_err(|_| ErrorData::internal_error("MCP sampling response invalid", None))
	}
	async fn create_elicitation(
		&self,
		params: ElicitRequestParams,
		context: RequestContext<RoleClient>,
	) -> std::result::Result<ElicitResult, ErrorData> {
		let raw = serde_json::to_value(params)
			.map_err(|_| ErrorData::invalid_params("MCP_ELICITATION_INVALID", None))?;
		let value = self
			.request(context.id.to_string(), "elicitation/create", raw)
			.await?;
		serde_json::from_value(value)
			.map_err(|_| ErrorData::internal_error("MCP elicitation response invalid", None))
	}
}

const IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);
const MAX_SESSIONS: usize = 512;
const MAX_PAGES: usize = 10;
const MAX_LISTED_SCHEMA_BYTES: usize = 1 << 20;

mod wire;
use wire::{BoundedClient, call_error, initialization_error};

type Service = RunningService<RoleClient, Inputs>;
type Failure = StreamableHttpError<reqwest::Error>;
type Attempt<T> = std::result::Result<T, Fault>;

/// Every Run of the process shares one instance, through an `Arc`.
pub struct McpPluginSessions {
	origins: KeylessAccess,
	clients: ToolClients,
	state: Arc<Mutex<State>>,
}

const _: () = {
	const fn shared<T: Send + Sync>() {}
	shared::<McpPluginSessions>();
};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
	run: Uuid,
	plugin: PluginName,
	digest: String,
	server: ServerName,
}

impl Key {
	fn new(run: Uuid, server: &PluginServer) -> Self {
		Self {
			run,
			plugin: server.plugin.clone(),
			digest: server.digest.clone(),
			server: server.server.clone(),
		}
	}
}

struct Entry {
	service: Arc<Service>,
	used: Instant,
}

/// Only synchronous methods touch it, so the lock is never held across network I/O.
struct State {
	entries: HashMap<Key, Entry>,
	idle: Duration,
	capacity: usize,
	reaping: bool,
}

impl State {
	/// The session if it is still usable, which counts as using it.
	fn live(&mut self, key: &Key) -> Option<Arc<Service>> {
		let entry = self.entries.get_mut(key)?;
		if (entry.used.elapsed() <= self.idle || Arc::strong_count(&entry.service) > 1)
			&& !entry.service.is_closed()
			&& !entry.service.is_transport_closed()
		{
			entry.used = Instant::now();
			return Some(entry.service.clone());
		}
		self.entries.remove(key);
		None
	}

	/// Dropping a session cancels it, and its worker then ends the server-side session.
	fn sweep(&mut self) {
		let idle = self.idle;
		self.entries.retain(|_, entry| {
			entry.used.elapsed() <= idle || Arc::strong_count(&entry.service) > 1
		});
	}

	/// Keeps `service` unless another call opened a session for `key` first, then makes room by
	/// dropping the least recently used sessions.
	fn adopt(&mut self, key: Key, service: Arc<Service>) -> Result<Arc<Service>> {
		self.sweep();
		if let Some(existing) = self.live(&key) {
			return Ok(existing);
		}
		while self.entries.len() >= self.capacity {
			let Some(oldest) = self
				.entries
				.iter()
				.filter(|(_, entry)| Arc::strong_count(&entry.service) == 1)
				.min_by_key(|(_, entry)| entry.used)
				.map(|(key, _)| key.clone())
			else {
				return Err(Error::External(
					"MCP session capacity is occupied by active calls".into(),
				));
			};
			self.entries.remove(&oldest);
		}
		self.entries.insert(
			key,
			Entry {
				service: service.clone(),
				used: Instant::now(),
			},
		);
		Ok(service)
	}

	fn touch(&mut self, key: &Key, service: &Arc<Service>) {
		if let Some(entry) = self
			.entries
			.get_mut(key)
			.filter(|entry| Arc::ptr_eq(&entry.service, service))
		{
			entry.used = Instant::now();
		}
	}

	/// Removes `service` only, never a newer session that took its place.
	fn forget(&mut self, key: &Key, service: &Arc<Service>) {
		if self
			.entries
			.get(key)
			.is_some_and(|entry| Arc::ptr_eq(&entry.service, service))
		{
			self.entries.remove(key);
		}
	}

	fn take_run(&mut self, run: Uuid) -> Vec<Arc<Service>> {
		self.sweep();
		let mut taken = Vec::new();
		self.entries.retain(|key, entry| {
			let ours = key.run == run;
			if ours {
				taken.push(entry.service.clone());
			}
			!ours
		});
		taken
	}
}

impl McpPluginSessions {
	pub fn new(origins: KeylessAccess, clients: ToolClients) -> Self {
		Self::with_limits(origins, clients, IDLE_LIMIT, MAX_SESSIONS)
	}

	fn with_limits(
		origins: KeylessAccess,
		clients: ToolClients,
		idle: Duration,
		capacity: usize,
	) -> Self {
		Self {
			origins,
			clients,
			state: Arc::new(Mutex::new(State {
				entries: HashMap::new(),
				idle,
				capacity: capacity.max(1),
				reaping: false,
			})),
		}
	}

	fn state(&self) -> MutexGuard<'_, State> {
		self.state.lock().unwrap_or_else(PoisonError::into_inner)
	}

	/// The Run's session with the server. A refused destination fails before any I/O.
	async fn session(&self, key: &Key, config: &RemoteServer) -> Attempt<Arc<Service>> {
		let live = self.state().live(key);
		if let Some(service) = live {
			return Ok(service);
		}
		let client = self
			.clients
			.select_mcp(&self.origins, config.url.as_str())
			.map_err(Fault::Early)?;
		let service = open(client, config).await?;
		let service = self
			.state()
			.adopt(key.clone(), Arc::new(service))
			.map_err(Fault::Early)?;
		self.expire_when_idle();
		Ok(service)
	}

	/// One deadline task closes quiet deployments too. Its weak reference keeps neither the owner
	/// nor any service alive, and it stops when the map empties.
	fn expire_when_idle(&self) {
		{
			let mut state = self.state();
			if state.reaping {
				return;
			}
			state.reaping = true;
		}
		let state = Arc::downgrade(&self.state);
		tokio::spawn(async move {
			loop {
				let delay = {
					let Some(state) = state.upgrade() else {
						return;
					};
					let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
					state.sweep();
					if state.entries.is_empty() {
						state.reaping = false;
						return;
					}
					state
						.entries
						.values()
						.filter(|entry| Arc::strong_count(&entry.service) == 1)
						.map(|entry| state.idle.saturating_sub(entry.used.elapsed()))
						.min()
						// Live calls and input waits retain their services. Completion refreshes
						// `used`, so checking within one idle interval cannot miss that deadline.
						.unwrap_or(state.idle)
						.max(Duration::from_millis(1))
				};
				tokio::time::sleep(delay).await;
			}
		});
	}
}

#[async_trait]
impl PluginSessions for McpPluginSessions {
	async fn list(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
	) -> Result<Vec<ListedTool>> {
		let key = Key::new(run, server);
		let service = self.session(&key, config).await.map_err(Fault::for_list)?;
		let result = list_tools(&service).await;
		self.state().touch(&key, &service);
		result.map_err(|fault| {
			// The Run pins nothing from a server that failed to list, so nothing will use the session.
			self.state().forget(&key, &service);
			fault.for_list()
		})
	}

	async fn call(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
		tool: &str,
		expected_digest: &str,
		arguments: Value,
		_key: &str,
	) -> Result<Value> {
		match timeout(
			TOOL_TIMEOUT,
			self.round(
				run,
				server,
				config,
				tool,
				expected_digest,
				arguments,
				_key,
				None,
				None,
			),
		)
		.await
		.map_err(|_| Fault::Timeout.for_call())??
		{
			ToolRound::Complete(value) => Ok(value),
			ToolRound::InputRequired(_) => Err(Error::Invalid(
				"MCP_INPUT_DENIED: interactive inputs are unavailable in this execution profile"
					.into(),
			)),
		}
	}

	async fn round(
		&self,
		run: Uuid,
		server: &PluginServer,
		config: &RemoteServer,
		tool: &str,
		expected_digest: &str,
		arguments: Value,
		_key: &str,
		checkpoint: Option<&CallCheckpoint>,
		inputs: Option<Arc<dyn PluginInputs>>,
	) -> Result<ToolRound> {
		let arguments = arguments
			.as_object()
			.cloned()
			.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
		let key = Key::new(run, server);
		let service = self.session(&key, config).await.map_err(Fault::for_call)?;
		if let Err(fault) = verify(&service, tool, expected_digest).await {
			self.state().forget(&key, &service);
			return Err(fault.for_call());
		}
		let _scope = service.service().enter(inputs)?;
		let mut request = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
		if let Some(checkpoint) = checkpoint
			&& let Some(input) = &checkpoint.input
		{
			if checkpoint.next_input().is_some() {
				return Err(Error::Invalid("MCP_INPUT_INCOMPLETE".into()));
			}
			request.request_state = input.request_state.clone();
			request.input_responses =
				(!input.requests.is_empty()).then(|| checkpoint.responses.clone());
		}
		// Session recovery never replays a sent tool request. A lost result is
		// reconciled through the invocation journal, including Read-only tools.
		let result = service.call_tool_once(request).await;
		self.state().touch(&key, &service);
		match result {
			Ok(CallToolResponse::Complete(result)) => {
				Ok(ToolRound::Complete(wire::result_value(result)?))
			}
			Ok(CallToolResponse::InputRequired(result)) => {
				let requests = result
					.input_requests
					.unwrap_or_default()
					.into_iter()
					.map(|(id, request)| {
						let raw = serde_json::to_value(request)?;
						Ok((
							id,
							InputRequest {
								method: raw["method"].as_str().unwrap_or_default().into(),
								params: raw["params"].clone(),
							},
						))
					})
					.collect::<Result<_>>()?;
				let input = RequiredInput {
					request_state: result.request_state,
					requests,
				};
				input.validate()?;
				Ok(ToolRound::InputRequired(input))
			}
			Ok(_) => Err(Error::Invalid("MCP_TASKS_UNSUPPORTED".into())),
			Err(error) => {
				self.state().forget(&key, &service);
				Err(call_error(&error))
			}
		}
	}

	async fn close(&self, run: Uuid) {
		let ended = self.state().take_run(run);
		futures_util::future::join_all(ended.into_iter().map(end)).await;
	}
}

/// Waits for the server-side session to end when no call still holds it; otherwise the last
/// holder's drop cancels it.
async fn end(service: Arc<Service>) {
	service.cancellation_token().cancel();
	if let Ok(service) = Arc::try_unwrap(service) {
		let _ = service.cancel().await;
	}
}

async fn open(client: &reqwest::Client, config: &RemoteServer) -> Attempt<Service> {
	let invalid = || Fault::Early(Error::Invalid("plugin server header is invalid".into()));
	let mut headers = HashMap::new();
	for header in &config.headers {
		headers.insert(
			HeaderName::from_bytes(header.name.as_bytes()).map_err(|_| invalid())?,
			HeaderValue::from_str(&header.value).map_err(|_| invalid())?,
		);
	}
	let mut transport_config =
		StreamableHttpClientTransportConfig::with_uri(config.url.as_str()).custom_headers(headers);
	transport_config.reinit_on_expired_session = false;
	transport_config.max_sse_event_size = 256_000;
	let transport =
		StreamableHttpClientTransport::with_client(BoundedClient(client.clone()), transport_config);
	timeout(
		TOOL_TIMEOUT,
		Inputs::default().serve_with_lifecycle(
			transport,
			ClientLifecycleMode::Auto {
				preferred_versions: vec![rmcp::model::ProtocolVersion::LATEST],
				legacy_version: Some(
					serde_json::from_value(serde_json::json!("2025-06-18"))
						.expect("supported legacy MCP revision"),
				),
			},
		),
	)
	.await
	.map_err(|_| Fault::Timeout)?
	.map_err(|error| Fault::Init(Box::new(error)))
}

/// Every tool of the server, within the page and size limits.
async fn list_tools(service: &Service) -> Attempt<Vec<ListedTool>> {
	let listing = async {
		let mut tools = Vec::new();
		let mut bytes = 0;
		let mut cursor = None;
		for _ in 0..MAX_PAGES {
			let page = service
				.list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
				.await
				.map_err(Fault::Service)?;
			for tool in page.tools {
				let input_schema = Value::Object((*tool.input_schema).clone());
				bytes += input_schema.to_string().len();
				if bytes > MAX_LISTED_SCHEMA_BYTES {
					return Err(Fault::Limit);
				}
				tools.push(ListedTool {
					name: tool.name.into_owned(),
					description: tool.description.map(std::borrow::Cow::into_owned),
					input_schema,
				});
			}
			cursor = page.next_cursor;
			if cursor.is_none() {
				return Ok(tools);
			}
		}
		Err(Fault::Limit)
	};
	timeout(TOOL_TIMEOUT, listing)
		.await
		.map_err(|_| Fault::Timeout)?
}

/// The tool the Run pinned must still be listed with the schema it pinned.
async fn verify(service: &Service, tool: &str, expected_digest: &str) -> Attempt<()> {
	let listed = list_tools(service).await?;
	match listed.iter().find(|listed| listed.name == tool) {
		Some(listed) if schema_digest(&listed.input_schema) == expected_digest => Ok(()),
		_ => Err(Fault::Early(Error::Invalid(format!(
			"CAPABILITY_CHANGED: {tool}"
		)))),
	}
}

/// What went wrong with a session, before the operation maps it to the port's errors.
enum Fault {
	/// Already an application error: a refused destination, an invalid argument or header, or a
	/// changed capability.
	Early(Error),
	Init(Box<ClientInitializeError>),
	Service(ServiceError),
	/// The server listed more pages or schema bytes than a Run takes in.
	Limit,
	Timeout,
}

impl Fault {
	/// A refused destination is [`Error::Forbidden`]; anything else is the cause code of a
	/// [`ListFailure`].
	fn for_list(self) -> Error {
		match self {
			Self::Early(error) => error,
			Self::Init(error) => match initialization_error(&error) {
				Error::Forbidden => Error::Forbidden,
				_ if unreachable(&error) => ListFailure::Unreachable.into(),
				_ => ListFailure::InitializeFailed.into(),
			},
			Self::Service(error) => match call_error(&error) {
				Error::Forbidden => Error::Forbidden,
				_ => service_failure(&error).into(),
			},
			Self::Limit => ListFailure::InvalidResponse.into(),
			Self::Timeout => ListFailure::Unreachable.into(),
		}
	}

	/// The mapping `HttpTools` gives an MCP call.
	fn for_call(self) -> Error {
		match self {
			Self::Early(error) => error,
			Self::Init(error) => initialization_error(&error),
			Self::Service(error) => call_error(&error),
			Self::Limit => Error::External("MCP tool list exceeds the listing limits".into()),
			Self::Timeout => Error::External("MCP request timed out".into()),
		}
	}
}

fn transport_failure(error: &ServiceError) -> Option<&Failure> {
	match error {
		ServiceError::TransportSend(source) => source.error.downcast_ref::<Failure>(),
		_ => None,
	}
}

/// The server could not be reached at all, as opposed to answering unusably.
fn unreachable(error: &ClientInitializeError) -> bool {
	if let ClientInitializeError::LegacyFallbackFailed { fallback, .. } = error {
		return unreachable(fallback);
	}
	let ClientInitializeError::TransportError { error, .. } = error else {
		return false;
	};
	match error.error.downcast_ref::<Failure>() {
		Some(Failure::Client(source)) => source.is_connect() || source.is_timeout(),
		Some(Failure::Io(_)) => true,
		_ => false,
	}
}

fn service_failure(error: &ServiceError) -> ListFailure {
	match error {
		ServiceError::TransportSend(_) => match transport_failure(error) {
			Some(
				Failure::UnexpectedServerResponse(_)
				| Failure::UnexpectedContentType(_)
				| Failure::Deserialize(_)
				| Failure::Sse(_),
			) => ListFailure::InvalidResponse,
			_ => ListFailure::Unreachable,
		},
		ServiceError::TransportClosed | ServiceError::Timeout { .. } => ListFailure::Unreachable,
		_ => ListFailure::InvalidResponse,
	}
}

#[cfg(test)]
mod tests;
