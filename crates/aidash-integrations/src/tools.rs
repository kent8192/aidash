//! HTTP and MCP transports for validated application tools.
use crate::{broker, keyless::Clients};
use aidash_application::execution::bounded_utf8_end;
use aidash_application::{
	Error, Result, ports::tools::ToolTransport, provider_access::KeylessAccess,
	tenant_secrets::CallSecret, tools::required,
};
use aidash_domain::{keyless::Destination, tool::ToolConfig};
use async_trait::async_trait;
use rmcp::{
	RoleClient, ServiceExt,
	model::{CallToolRequestParams, PaginatedRequestParams},
	service::RunningService,
	transport::{
		StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
	},
};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
pub(crate) mod auth;
pub(crate) mod mcp;

/// A broker-owned failure is only expected when this call is brokered. An
/// upstream response with the same header passthrough is the tool's own.
fn broker_failure(
	secret: Option<&CallSecret>,
	headers: &reqwest::header::HeaderMap,
) -> Option<broker::BrokerFailure> {
	match secret {
		Some(CallSecret::Brokered { .. } | CallSecret::BrokeredConnection { .. }) => {
			broker::BrokerFailure::from_headers(headers)
		}
		_ => None,
	}
}

fn is_brokered(secret: Option<&CallSecret>) -> bool {
	matches!(
		secret,
		Some(CallSecret::Brokered { .. } | CallSecret::BrokeredConnection { .. })
	)
}

/// A brokered call sends the value it does not have: connect failures, and a
/// response from the broker itself, are Deployment Administrator faults, never a direct retry.
fn brokered_send_error(error: reqwest::Error) -> Error {
	if crate::keyless::refused(&error) {
		crate::keyless::send_error(error)
	} else {
		broker::BrokerFailure::connect_failure().into_error()
	}
}

/// Total budget for a native `http_get`, covering DNS resolution and the request.
const NATIVE_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const TOOL_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Total budget for reading a server's tool list: handshake and every page.
const MCP_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Most `tools/list` pages read while looking for one tool.
const MCP_LIST_PAGES: usize = 20;

async fn resolve_within<I>(
	deadline: tokio::time::Instant,
	lookup: impl Future<Output = std::io::Result<I>>,
) -> Result<Vec<std::net::SocketAddr>>
where
	I: Iterator<Item = std::net::SocketAddr>,
{
	tokio::time::timeout_at(deadline, lookup)
		.await
		.map_err(|_| Error::External("HTTP tool DNS resolution timed out".into()))?
		.map(Iterator::collect)
		.map_err(|error| Error::Port(Box::new(error)))
}

/// The clients of Tenant tool endpoints. Neither follows redirects or uses a
/// system proxy: a proxy resolves names itself, which would bypass the
/// address check of `public`.
#[derive(Clone)]
pub struct ToolClients {
	/// An origin the Deployment permits, which may reach private addresses.
	permitted: reqwest::Client,
	/// Every other endpoint: each new connection resolves public addresses only.
	public: reqwest::Client,
	mcp_permitted: reqwest::Client,
	mcp_public: reqwest::Client,
}
impl ToolClients {
	pub fn new() -> Result<Self> {
		Ok(Self {
			mcp_public: crate::keyless::public_client_timeout(None, TOOL_CONNECT_TIMEOUT)?,
			mcp_permitted: reqwest::Client::builder()
				.connect_timeout(TOOL_CONNECT_TIMEOUT)
				.redirect(reqwest::redirect::Policy::none())
				.no_proxy()
				.build()
				.map_err(crate::http_error)?,
			permitted: reqwest::Client::builder()
				.timeout(TOOL_TIMEOUT)
				.connect_timeout(TOOL_CONNECT_TIMEOUT)
				.redirect(reqwest::redirect::Policy::none())
				.no_proxy()
				.build()
				.map_err(crate::http_error)?,
			public: crate::keyless::public_client(TOOL_TIMEOUT, TOOL_CONNECT_TIMEOUT)?,
		})
	}
	/// The client of an Agent Plugin MCP session with `endpoint`. The endpoint must pass the
	/// destination rule either way, but a brokered connection sends every byte to the broker,
	/// which is Deployment configuration and may be an in-cluster private address, exactly as
	/// registry-tool dispatch does.
	pub(crate) fn select_mcp(
		&self,
		origins: &KeylessAccess,
		endpoint: &str,
		auth: Option<&CallSecret>,
	) -> Result<&reqwest::Client> {
		let client = match tool_destination(origins, endpoint)? {
			Destination::Permitted => &self.mcp_permitted,
			Destination::PublicOnly => &self.mcp_public,
			Destination::Forbidden => return Err(Error::Forbidden),
		};
		// Only a brokered connection is routed to the broker by the plugin transport.
		Ok(
			if matches!(auth, Some(CallSecret::BrokeredConnection { .. })) {
				&self.mcp_permitted
			} else {
				client
			},
		)
	}

	/// The client a call to `endpoint` may use; a refused destination never reaches I/O.
	pub(crate) fn select(
		&self,
		origins: &KeylessAccess,
		endpoint: &str,
	) -> Result<&reqwest::Client> {
		match tool_destination(origins, endpoint)? {
			Destination::Permitted => Ok(&self.permitted),
			Destination::PublicOnly => Ok(&self.public),
			Destination::Forbidden => {
				tracing::warn!("tool endpoint refused: its host is a non-public address");
				Err(Error::Forbidden)
			}
		}
	}
}

/// Invocation and pre-registration discovery apply the same origin/TLS rule.
fn tool_destination(origins: &KeylessAccess, endpoint: &str) -> Result<Destination> {
	let destination = origins.destination(endpoint)?;
	if destination == Destination::PublicOnly {
		let url = reqwest::Url::parse(endpoint)
			.map_err(|_| Error::Invalid("invalid tool endpoint URL".into()))?;
		if url.scheme() != "https" {
			return Err(Error::Forbidden);
		}
	}
	Ok(destination)
}

/// A Tenant chooses a tool's endpoint, so the Keyless endpoint rule decides
/// where each call may connect (ADR 0036).
#[derive(Clone)]
pub struct HttpTools {
	origins: KeylessAccess,
	clients: ToolClients,
}
impl HttpTools {
	pub fn new(origins: KeylessAccess, clients: ToolClients) -> Self {
		Self { origins, clients }
	}
	fn client(&self, endpoint: &str) -> Result<&reqwest::Client> {
		self.clients.select(&self.origins, endpoint)
	}
}
#[async_trait]
impl ToolTransport for HttpTools {
	async fn invoke(
		&self,
		configuration: &ToolConfig,
		input: Value,
		key: &str,
		secret: Option<&CallSecret>,
	) -> Result<Value> {
		match configuration {
			ToolConfig::Native {
				operation,
				allowed_hosts,
			} if operation == "http_get" => {
				let url = aidash_application::tools::validate_native_http_url(
					required(&input, "url")?,
					allowed_hosts,
				)?;
				let host = url.host_str().ok_or(Error::Forbidden)?;
				// One deadline bounds resolution and the pinned request together.
				let deadline = tokio::time::Instant::now() + NATIVE_HTTP_TIMEOUT;
				let addresses =
					resolve_within(deadline, tokio::net::lookup_host((host, 443))).await?;
				if addresses.is_empty()
					|| addresses.len() > 32
					|| addresses.iter().any(|address| {
						!aidash_domain::capabilities::outbound::public_ip(address.ip())
					}) {
					return Err(Error::Forbidden);
				}
				let client = reqwest::Client::builder()
					.no_proxy()
					.redirect(reqwest::redirect::Policy::none())
					.timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
					.resolve_to_addrs(host, &addresses)
					.build()
					.map_err(crate::http_error)?;
				let response = client
					.get(url.clone())
					.send()
					.await
					.map_err(crate::http_error)?;
				web_fetch_response(response, &url).await
			}
			ToolConfig::Http {
				endpoint, replay, ..
			} => {
				// Keep the destination authority check even when the request goes
				// to the broker: the endpoint must still be one this call may use.
				let endpoint_client = match self.client(endpoint) {
					Ok(client) => client,
					Err(Error::Forbidden) => return Ok(tool_policy_refusal()),
					Err(error) => return Err(error),
				};
				let request = if let Some(
					CallSecret::Brokered { source, rest, .. }
					| CallSecret::BrokeredConnection { source, rest, .. },
				) = secret
				{
					// The broker is Deployment configuration, not the Tenant's
					// choice, so it may be an in-cluster private address.
					// A broker-owned `connection_required` (the grant ended at refresh) is
					// typed here, never inferred from a body.
					let route = source
						.route()
						.await
						.map_err(|error| auth::source_error(error, endpoint))?;
					let url = broker::forward_url(&route.base, rest)?;
					self.clients
						.permitted
						.post(url)
						.header(broker::SECRET_CALL, route.token.expose_secret())
						.header("idempotency-key", key)
						.json(&input)
				} else {
					let request = endpoint_client
						.post(endpoint)
						.header("idempotency-key", key)
						.json(&input);
					if let Some(CallSecret::Value { value, .. }) = secret {
						// A self-hosted Tenant Secret goes as the bearer
						// credential, exactly as before broker deployments.
						request.bearer_auth(value.expose_secret())
					} else {
						request
					}
				};
				let mut response = match request.send().await {
					Ok(response) => response,
					Err(error) if crate::keyless::refused(&error) => {
						return Ok(tool_policy_refusal());
					}
					Err(error) if is_brokered(secret) => return Err(brokered_send_error(error)),
					Err(error) if error.is_connect() => {
						return Err(Error::ToolNotDelivered {
							message: format!("Tool connection failed: {}", error.without_url()),
							retry_after: None,
						});
					}
					Err(error) => return Err(crate::keyless::send_error(error)),
				};
				if let Some(failure) = broker_failure(secret, response.headers()) {
					return Err(failure.into_error());
				}
				if let Some(error) = auth::challenge(&response, endpoint) {
					return Err(auth::finish(error));
				}
				let status = response.status();
				let unsafe_failure = status.is_server_error()
					&& !matches!(replay.as_str(), "read_only" | "idempotent");
				if matches!(status.as_u16(), 408 | 429)
					|| (status.is_server_error() && !unsafe_failure)
				{
					let retry_after = tool_retry_after(response.headers());
					return Err(Error::ToolNotDelivered {
						message: format!("HTTP tool returned {status}"),
						retry_after,
					});
				}
				if (status.is_client_error() && !matches!(status.as_u16(), 408 | 429))
					|| unsafe_failure
				{
					let mut body = Vec::new();
					while body.len() < 4096 {
						let Ok(Some(chunk)) = response.chunk().await else {
							break;
						};
						let remaining = 4096 - body.len();
						body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
					}
					let mut error = json!({"kind":"http_status","status":status.as_u16(),"body":bounded_tool_body(&body)});
					if unsafe_failure {
						error["message"] = json!("The change may have started.");
					}
					return Ok(json!({"error":error}));
				}
				if !response.status().is_success() {
					return Err(Error::External(format!(
						"HTTP tool returned {}",
						response.status()
					)));
				}
				Ok(crate::response::json(response, 256_000)
					.await
					.unwrap_or_else(
						|_| json!({"message":"The call succeeded but its result could not be read."}),
					))
			}
			ToolConfig::Mcp {
				endpoint,
				tool_name,
				idempotency_argument,
				replay,
				..
			} => {
				let client = match self.client(endpoint) {
					Ok(client) => client,
					Err(error) => {
						return tool_service_failure(
							error,
							matches!(replay.as_str(), "read_only" | "idempotent"),
						);
					}
				};
				let client = if is_brokered(secret) {
					&self.clients.permitted
				} else {
					client
				};
				let mut args = input
					.as_object()
					.cloned()
					.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
				if let Some(argument) = idempotency_argument {
					args.insert(argument.clone(), json!(key));
				}
				let mut result = mcp_call(
					client,
					endpoint,
					tool_name,
					args.clone(),
					secret,
					matches!(replay.as_str(), "read_only" | "idempotent"),
				)
				.await;
				if let Err(error) = &result
					&& auth::retry(error, secret).await
				{
					result = mcp_call(
						client,
						endpoint,
						tool_name,
						args,
						secret,
						matches!(replay.as_str(), "read_only" | "idempotent"),
					)
					.await;
				}
				match result {
					Err(error) => {
						let error = auth::finish(error);
						match auth::error_value(&error) {
							Some(value) => Ok(value),
							None => tool_service_failure(
								error,
								matches!(replay.as_str(), "read_only" | "idempotent"),
							),
						}
					}
					value => value,
				}
			}
			_ => Err(Error::Invalid(
				"tool configuration has no external transport".into(),
			)),
		}
	}
}
async fn mcp_call(
	client: &reqwest::Client,
	endpoint: &str,
	tool: &str,
	arguments: serde_json::Map<String, Value>,
	secret: Option<&CallSecret>,
	replay_safe: bool,
) -> Result<Value> {
	auth::validate(secret, endpoint)?;
	let service = match mcp_session(client, endpoint, secret).await {
		Ok(service) => service,
		Err(error) => return tool_setup_failure(error, replay_safe),
	};
	let result = service
		.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
		.await;
	let _ = service.cancel().await;
	match result {
		Ok(value) => mcp::result_value(value),
		Err(rmcp::service::ServiceError::McpError(error)) => mcp_protocol_result(&error.message),
		Err(error) => Err(mcp::call_error(&error)),
	}
}

pub(crate) fn tool_policy_refusal() -> Value {
	json!({"error":{"kind":"outbound_policy","message":"The deployment's outbound policy refused this destination."}})
}
pub(crate) fn mcp_protocol_failure(message: &str) -> Value {
	json!({"error":{"kind":"mcp_error","message":bounded_tool_body(message.as_bytes())}})
}
pub(crate) fn mcp_protocol_result(message: &str) -> Result<Value> {
	match mcp_sse_outcome(message) {
		Some(true) => Ok(json!({"message":"The call succeeded but its result could not be read."})),
		Some(false) => Err(Error::External("MCP answer was lost after send".into())),
		None if message.starts_with("aidash-internal-sse-") => {
			Err(Error::External("MCP answer was lost after send".into()))
		}
		None => Ok(mcp_protocol_failure(message)),
	}
}

fn bounded_tool_body(bytes: &[u8]) -> String {
	let decoded = String::from_utf8_lossy(bytes);
	decoded[..bounded_utf8_end(&decoded, 0, 4096)].to_owned()
}

fn tool_retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
	let value = headers
		.get(reqwest::header::RETRY_AFTER)?
		.to_str()
		.ok()?
		.trim();
	if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	let seconds = value.bytes().fold(0_u64, |seconds, byte| {
		(seconds * 10 + u64::from(byte - b'0')).min(32)
	});
	Some(std::time::Duration::from_secs(seconds))
}

pub(crate) fn tool_service_failure(error: Error, replay_safe: bool) -> Result<Value> {
	match error {
		Error::ToolServiceStatus {
			status,
			body,
			retry_after,
		} => {
			if (200..300).contains(&status) {
				Ok(json!({"message":"The call succeeded but its result could not be read."}))
			} else if matches!(status, 408 | 429) || (status >= 500 && replay_safe) {
				Err(Error::ToolNotDelivered {
					message: format!("MCP tool returned HTTP {status}"),
					retry_after,
				})
			} else {
				let mut detail = json!({"kind":"http_status","status":status,"body":body});
				if status >= 500 {
					detail["message"] = json!("The change may have started.");
				}
				Ok(json!({"error":detail}))
			}
		}
		Error::Forbidden => Ok(tool_policy_refusal()),
		other => Err(other),
	}
}

/// Session setup and tool-list verification precede `tools/call`, so a lost
/// setup answer cannot make the tool's effect uncertain.
pub(crate) fn tool_setup_failure(error: Error, replay_safe: bool) -> Result<Value> {
	match error {
		Error::External(message) => Err(Error::ToolNotDelivered {
			message,
			retry_after: None,
		}),
		other => tool_service_failure(other, replay_safe),
	}
}

const MCP_HTTP_STATUS_PREFIX: &str = "aidash-mcp-http-status:";
pub(crate) fn mcp_unreadable() -> String {
	format!(
		"{MCP_HTTP_STATUS_PREFIX}{}",
		json!({"status":200,"retry_after":null,"body":""})
	)
}

/// A bounded prefix proves a final successful JSON-RPC response only after it
/// names the request id and the top-level `result` field. An incomplete or
/// unrecognizable response remains uncertain.
fn mcp_final_prefix_id(bytes: &[u8]) -> Option<Value> {
	use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
	struct Probe {
		version: bool,
		version_seen: bool,
		id: Option<Value>,
		result: bool,
		other_kind: bool,
	}
	impl<'de> Visitor<'de> for &mut Probe {
		type Value = ();
		fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			formatter.write_str("a JSON-RPC response")
		}
		fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<(), M::Error> {
			while let Some(key) = map.next_key::<String>()? {
				match key.as_str() {
					"jsonrpc" => {
						self.other_kind |= self.version_seen;
						self.version_seen = true;
						self.version = map.next_value::<String>()? == "2.0";
					}
					"id" => {
						self.other_kind |= self.id.is_some();
						self.id = Some(map.next_value::<Value>()?);
					}
					"result" => {
						self.other_kind |= self.result;
						self.result = true;
						let _: IgnoredAny = map.next_value()?;
					}
					"method" | "error" => {
						self.other_kind = true;
						let _: IgnoredAny = map.next_value()?;
					}
					_ => {
						let _: IgnoredAny = map.next_value()?;
					}
				}
			}
			Ok(())
		}
	}
	struct Seed<'a>(&'a mut Probe);
	impl<'de> DeserializeSeed<'de> for Seed<'_> {
		type Value = ();
		fn deserialize<D: serde::Deserializer<'de>>(
			self,
			deserializer: D,
		) -> std::result::Result<(), D::Error> {
			deserializer.deserialize_map(self.0)
		}
	}
	let mut probe = Probe {
		version: false,
		version_seen: false,
		id: None,
		result: false,
		other_kind: false,
	};
	let _ = Seed(&mut probe).deserialize(&mut serde_json::Deserializer::from_slice(bytes));
	if probe.version && probe.result && !probe.other_kind {
		probe
			.id
			.filter(|id| id.is_string() || id.is_i64() || id.is_u64())
	} else {
		None
	}
}

pub(crate) async fn mcp_json_body(
	mut response: reqwest::Response,
	request_id: Option<&Value>,
	limit: usize,
) -> std::result::Result<Value, String> {
	let mut bytes = Vec::new();
	let mut oversized = false;
	loop {
		match response.chunk().await {
			Ok(Some(chunk)) => {
				let remaining = limit.saturating_sub(bytes.len());
				bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
				if chunk.len() > remaining {
					oversized = true;
				}
			}
			Ok(None) => {
				if !oversized && let Ok(value) = serde_json::from_slice(&bytes) {
					return Ok(value);
				}
				break;
			}
			Err(_) => return Err("MCP answer was lost after send".into()),
		}
	}
	Err(
		if request_id.is_some() && mcp_final_prefix_id(&bytes).as_ref() == request_id {
			mcp_unreadable()
		} else {
			"MCP answer could not be read".into()
		},
	)
}

pub(crate) fn mcp_result_identified(value: &Value, request_id: Option<&Value>) -> bool {
	value.get("jsonrpc") == Some(&json!("2.0"))
		&& request_id.is_some()
		&& value.get("id") == request_id
		&& value.get("result").is_some()
		&& value.get("error").is_none()
		&& value.get("method").is_none()
}

/// The SDK consumes an SSE stream asynchronously. A one-use, unguessable
/// marker carries an oversized stream's outcome back through its JSON-RPC
/// response without trusting a marker a remote server could invent.
type McpSseMarkers =
	std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, bool)>>;
type McpSseSignal = std::sync::Arc<std::sync::Mutex<Option<(Value, bool)>>>;
type McpSseStream = futures_util::stream::BoxStream<
	'static,
	std::result::Result<sse_stream::Sse, sse_stream::Error>,
>;
fn mcp_sse_markers() -> &'static McpSseMarkers {
	static MARKERS: std::sync::OnceLock<McpSseMarkers> = std::sync::OnceLock::new();
	MARKERS.get_or_init(Default::default)
}
pub(crate) fn mcp_sse_marker(id: &Value, final_result: bool) -> Option<sse_stream::Sse> {
	let mut markers = mcp_sse_markers().lock().ok()?;
	markers.retain(|_, (at, _)| at.elapsed() < std::time::Duration::from_secs(300));
	if markers.len() >= 10_000 {
		return None;
	}
	let token = uuid::Uuid::new_v4().to_string();
	markers.insert(token.clone(), (std::time::Instant::now(), final_result));
	Some(sse_stream::Sse::default().data(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":format!("aidash-internal-sse-{token}")}}).to_string()))
}
pub(crate) fn mcp_sse_outcome(message: &str) -> Option<bool> {
	let token = message.strip_prefix("aidash-internal-sse-")?;
	mcp_sse_markers()
		.lock()
		.ok()?
		.remove(token)
		.map(|(_, final_result)| final_result)
}
pub(crate) fn mcp_sse_signal(bytes: &[u8], expected: Option<&Value>) -> Option<(Value, bool)> {
	let text = String::from_utf8_lossy(bytes);
	let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
	let current = normalized.rsplit("\n\n").next().unwrap_or_default();
	let current = current.strip_prefix('\u{feff}').unwrap_or(current);
	let mut event_name = None;
	for line in current.lines() {
		if let Some(value) = line.strip_prefix("event:") {
			if event_name
				.replace(value.strip_prefix(' ').unwrap_or(value))
				.is_some()
			{
				return expected.cloned().map(|id| (id, false));
			}
		} else if !line.starts_with(':')
			&& !line.starts_with("data:")
			&& !line.starts_with("id:")
			&& !line.starts_with("retry:")
		{
			return expected.cloned().map(|id| (id, false));
		}
	}
	if event_name.is_some_and(|name| !name.is_empty() && name != "message") {
		return expected.cloned().map(|id| (id, false));
	}
	let data = current
		.lines()
		.filter_map(|line| {
			line.strip_prefix("data:")
				.map(|data| data.strip_prefix(' ').unwrap_or(data))
		})
		.collect::<Vec<_>>()
		.join("\n");
	if let Some(id) = mcp_final_prefix_id(data.as_bytes())
		&& expected.is_none_or(|value| *value == id)
	{
		Some((id, true))
	} else {
		expected.cloned().map(|id| (id, false))
	}
}

/// Drain the remainder of an oversized SSE event without buffering its body.
/// A complete blank-line terminator is required before an identified result
/// is accepted; trailing control fields make the outcome uncertain.
pub(crate) async fn mcp_sse_complete(response: &mut reqwest::Response, first: &[u8]) -> bool {
	struct Tail {
		partial: bool,
		skip_lf: bool,
		line: Vec<u8>,
		valid: bool,
	}
	impl Tail {
		fn feed(&mut self, bytes: &[u8]) -> Option<bool> {
			for &byte in bytes {
				if std::mem::take(&mut self.skip_lf) && byte == b'\n' {
					continue;
				}
				if matches!(byte, b'\r' | b'\n') {
					self.skip_lf = byte == b'\r';
					if self.partial {
						self.partial = false;
						self.line.clear();
						continue;
					}
					if self.line.is_empty() {
						return Some(self.valid);
					}
					if !(self.line.starts_with(b"data:") || self.line.starts_with(b":")) {
						self.valid = false;
					}
					self.line.clear();
				} else if !self.partial && self.line.len() < 32 {
					self.line.push(byte);
				}
			}
			None
		}
	}
	let mut tail = Tail {
		partial: true,
		skip_lf: false,
		line: Vec::new(),
		valid: true,
	};
	if let Some(result) = tail.feed(first) {
		return result;
	}
	loop {
		match response.chunk().await {
			Ok(Some(chunk)) => {
				if let Some(result) = tail.feed(&chunk) {
					return result;
				}
			}
			Ok(None) | Err(_) => return false,
		}
	}
}

pub(crate) fn mcp_sse_wrap(stream: McpSseStream, signal: McpSseSignal) -> McpSseStream {
	use futures_util::StreamExt;
	stream
		.map(move |item| {
			if item.is_err()
				&& let Some((id, final_result)) =
					signal.lock().ok().and_then(|mut state| state.take())
				&& let Some(marker) = mcp_sse_marker(&id, final_result)
			{
				return Ok(marker);
			}
			item
		})
		.boxed()
}
pub(crate) async fn mcp_http_status(mut response: reqwest::Response) -> String {
	let status = response.status().as_u16();
	let retry_after = tool_retry_after(response.headers()).map(|delay| delay.as_secs());
	let mut bytes = Vec::new();
	while bytes.len() < 4096 {
		let Ok(Some(chunk)) = response.chunk().await else {
			break;
		};
		let remaining = 4096 - bytes.len();
		bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
	}
	format!(
		"{MCP_HTTP_STATUS_PREFIX}{}",
		json!({"status":status,"retry_after":retry_after,"body":bounded_tool_body(&bytes)})
	)
}
pub(crate) fn decode_mcp_http_status(message: &str) -> Option<Error> {
	let value: Value = serde_json::from_str(message.strip_prefix(MCP_HTTP_STATUS_PREFIX)?).ok()?;
	Some(Error::ToolServiceStatus {
		status: u16::try_from(value["status"].as_u64()?).ok()?,
		body: value["body"].as_str()?.to_owned(),
		retry_after: value["retry_after"]
			.as_u64()
			.map(std::time::Duration::from_secs),
	})
}
/// An initialized session with the MCP server at `endpoint`, over the bounded
/// transport. Invocation and the registration form's hint share it, so both
/// share the same bounded transport. Only invocation may attach a Tenant Secret.
async fn mcp_session(
	client: &reqwest::Client,
	endpoint: &str,
	secret: Option<&CallSecret>,
) -> Result<RunningService<RoleClient, ()>> {
	let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint.to_owned());
	let client = match secret {
		// A brokered session sends every request to the broker and carries a
		// fresh Tenant Secret Capability Token instead of a bearer value.
		Some(
			CallSecret::Brokered { source, rest, .. }
			| CallSecret::BrokeredConnection { source, rest, .. },
		) => {
			let route = source
				.route()
				.await
				.map_err(|error| auth::source_error(error, endpoint))?;
			mcp::BoundedClient::brokered(client.clone(), source.clone(), rest.clone(), route)
		}
		Some(CallSecret::Value { value, .. }) => {
			config = config.auth_header(value.expose_secret());
			mcp::BoundedClient::direct(client.clone())
		}
		Some(CallSecret::Connection { source, .. }) => {
			mcp::BoundedClient::connection(client.clone(), source.clone())
		}
		None => mcp::BoundedClient::direct(client.clone()),
	};
	let transport = StreamableHttpClientTransport::with_client(client, config);
	().serve(transport)
		.await
		.map_err(|e| mcp::initialization_error(&e))
}

/// What the MCP server at `endpoint` says about `tool_name` in `tools/list`:
/// its `annotations.readOnlyHint`, or `None` when it gives none. The answer only
/// prefills the registration form. It never makes an operation read-only
/// (ADR 0078), so nothing at run time reads it.
///
/// The Tenant chooses `endpoint` before any approval, so it is reached like a
/// Keyless endpoint (ADR 0036): public addresses only, or an origin the
/// Deployment permits, without redirects. A refused address is `Forbidden`.
pub async fn mcp_read_only_hint(
	clients: &Clients,
	origins: &KeylessAccess,
	endpoint: &str,
	tool_name: &str,
) -> Result<Option<bool>> {
	crate::validate_endpoint(endpoint)?;
	let client = clients.keyless(tool_destination(origins, endpoint)?)?;
	// One budget covers the handshake and every page; dropping the session on
	// timeout closes it.
	tokio::time::timeout(MCP_LIST_TIMEOUT, async {
		let service = mcp_session(client, endpoint, None)
			.await
			.map_err(|error| match error {
				error @ (Error::ToolNotDelivered { .. } | Error::ToolServiceStatus { .. }) => {
					Error::External(error.to_string())
				}
				other => other,
			})?;
		let hint = listed_hint(&service, tool_name).await;
		let _ = service.cancel().await;
		hint
	})
	.await
	.map_err(|_| Error::External("MCP tool listing timed out".into()))?
}

/// Page through `tools/list` until `tool_name` appears, reading at most
/// `MCP_LIST_PAGES` pages so a server cannot keep the request open.
async fn listed_hint(
	service: &RunningService<RoleClient, ()>,
	tool_name: &str,
) -> Result<Option<bool>> {
	let mut cursor = None;
	for _ in 0..MCP_LIST_PAGES {
		let page = service
			.list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
			.await
			.map_err(|e| {
				Error::External(format!("MCP tool listing failed: {}", mcp::call_error(&e)))
			})?;
		if let Some(tool) = page.tools.into_iter().find(|tool| tool.name == tool_name) {
			return Ok(tool
				.annotations
				.and_then(|annotations| annotations.read_only_hint));
		}
		cursor = page.next_cursor;
		if cursor.is_none() {
			break;
		}
	}
	Err(Error::Invalid(
		"the MCP server lists no tool with that name".into(),
	))
}

async fn web_fetch_response(mut response: reqwest::Response, url: &reqwest::Url) -> Result<Value> {
	let status = response.status();
	if matches!(
		status,
		reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND
	) {
		// These terminal source rejections are evidence the agent can work around.
		// Other errors, including transient 408/429 responses, retain worker retries.
		let url = &url.as_str()[..bounded_utf8_end(url.as_str(), 0, 2048)];
		return Ok(
			json!({"ok":false,"error":{"kind":"http_status","url":url,"status":status.as_u16()}}),
		);
	}
	response.error_for_status_ref().map_err(crate::http_error)?;
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(crate::http_error)? {
		if bytes.len() + chunk.len() > 256_000 {
			return Err(Error::Invalid("tool response exceeds 256 KB".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(json!({"text":String::from_utf8_lossy(&bytes)}))
}

#[cfg(test)]
mod tests;
