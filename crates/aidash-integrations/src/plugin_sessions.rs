//! A Run's MCP sessions with the remote servers of its Agent Plugins (ADR 0073). A session
//! outlives the harness step that opened it, so the sessions live in one process-wide map that
//! opens a missing entry on demand and gives up idle ones.
use crate::{
	Error, Result,
	tools::{
		TOOL_TIMEOUT, ToolClients,
		mcp::{BoundedClient, call_error, initialization_error, result_value},
	},
};
use aidash_application::{
	ports::tools::{ListFailure, PluginServer, PluginSessions},
	provider_access::KeylessAccess,
};
use aidash_domain::agent_plugin::{
	ListedTool, PluginName, RemoteServer, ServerName, schema_digest,
};
use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
	RoleClient, ServiceExt,
	model::{CallToolRequestParams, PaginatedRequestParams},
	service::{ClientInitializeError, RunningService, ServiceError},
	transport::{
		StreamableHttpClientTransport,
		streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
	},
};
use serde_json::Value;
use std::{
	collections::HashMap,
	sync::{Arc, Mutex, MutexGuard, PoisonError},
	time::{Duration, Instant},
};
use tokio::time::timeout;
use uuid::Uuid;

const IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);
const MAX_SESSIONS: usize = 512;
const MAX_PAGES: usize = 10;
const MAX_LISTED_SCHEMA_BYTES: usize = 1 << 20;

type Service = RunningService<RoleClient, ()>;
type Failure = StreamableHttpError<reqwest::Error>;
type Attempt<T> = std::result::Result<T, Fault>;

/// Every Run of the process shares one instance, through an `Arc`.
pub struct McpPluginSessions {
	origins: KeylessAccess,
	clients: ToolClients,
	state: Mutex<State>,
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
}

impl State {
	/// The session if it is still usable, which counts as using it.
	fn live(&mut self, key: &Key) -> Option<Arc<Service>> {
		let entry = self.entries.get_mut(key)?;
		if entry.used.elapsed() <= self.idle
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
		self.entries.retain(|_, entry| entry.used.elapsed() <= idle);
	}

	/// Keeps `service` unless another call opened a session for `key` first, then makes room by
	/// dropping the least recently used sessions.
	fn adopt(&mut self, key: Key, service: Arc<Service>) -> Arc<Service> {
		self.sweep();
		if let Some(existing) = self.live(&key) {
			return existing;
		}
		while self.entries.len() >= self.capacity {
			let Some(oldest) = self
				.entries
				.iter()
				.min_by_key(|(_, entry)| entry.used)
				.map(|(key, _)| key.clone())
			else {
				break;
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
		service
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
			state: Mutex::new(State {
				entries: HashMap::new(),
				idle,
				capacity: capacity.max(1),
			}),
		}
	}

	fn state(&self) -> MutexGuard<'_, State> {
		self.state.lock().unwrap_or_else(PoisonError::into_inner)
	}

	/// The Run's session with the server, and whether this call opened it. A refused destination
	/// fails before any I/O.
	async fn session(&self, key: &Key, config: &RemoteServer) -> Attempt<(Arc<Service>, bool)> {
		let live = self.state().live(key);
		if let Some(service) = live {
			return Ok((service, false));
		}
		let client = self
			.clients
			.select(&self.origins, config.url.as_str())
			.map_err(Fault::Early)?;
		let service = open(client, config).await?;
		Ok((self.state().adopt(key.clone(), Arc::new(service)), true))
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
		let (service, _) = self.session(&key, config).await.map_err(Fault::for_list)?;
		list_tools(&service).await.map_err(|fault| {
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
		let arguments = arguments
			.as_object()
			.cloned()
			.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
		let key = Key::new(run, server);
		let mut retried = false;
		loop {
			let (service, opened) = self.session(&key, config).await.map_err(Fault::for_call)?;
			if opened && let Err(fault) = verify(&service, tool, expected_digest).await {
				// A session kept after a refusal would let the next call skip the check.
				self.state().forget(&key, &service);
				return Err(fault.for_call());
			}
			let request =
				CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments.clone());
			match timeout(TOOL_TIMEOUT, service.call_tool(request)).await {
				Ok(Ok(result)) => return result_value(result),
				// Only a request the server rejected for its session was never run, so only
				// that one is safe to send again; any other failure may have run the tool.
				Ok(Err(error)) if !retried && session_expired(&error) => {
					retried = true;
					self.state().forget(&key, &service);
				}
				Ok(Err(error)) => return Err(call_error(&error)),
				Err(_) => {
					self.state().forget(&key, &service);
					return Err(Fault::Timeout.for_call());
				}
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
	let transport = StreamableHttpClientTransport::with_client(
		BoundedClient(client.clone()),
		StreamableHttpClientTransportConfig::with_uri(config.url.as_str()).custom_headers(headers),
	);
	timeout(TOOL_TIMEOUT, ().serve(transport))
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

fn session_expired(error: &ServiceError) -> bool {
	matches!(transport_failure(error), Some(Failure::SessionExpired))
}

/// The server could not be reached at all, as opposed to answering unusably.
fn unreachable(error: &ClientInitializeError) -> bool {
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
