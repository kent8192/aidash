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
		Some(CallSecret::Brokered { .. }) => broker::BrokerFailure::from_headers(headers),
		_ => None,
	}
}

fn is_brokered(secret: Option<&CallSecret>) -> bool {
	matches!(secret, Some(CallSecret::Brokered { .. }))
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
	pub(crate) fn select_mcp(
		&self,
		origins: &KeylessAccess,
		endpoint: &str,
	) -> Result<&reqwest::Client> {
		match tool_destination(origins, endpoint)? {
			Destination::Permitted => Ok(&self.mcp_permitted),
			Destination::PublicOnly => Ok(&self.mcp_public),
			Destination::Forbidden => Err(Error::Forbidden),
		}
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
			ToolConfig::Http { endpoint, .. } => {
				// Keep the destination authority check even when the request goes
				// to the broker: the endpoint must still be one this call may use.
				let endpoint_client = self.client(endpoint)?;
				let request = if let Some(CallSecret::Brokered { source, rest, .. }) = secret {
					// The broker is Deployment configuration, not the Tenant's
					// choice, so it may be an in-cluster private address.
					let route = source.route().await?;
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
				let response = request.send().await.map_err(|error| {
					if is_brokered(secret) {
						brokered_send_error(error)
					} else {
						crate::keyless::send_error(error)
					}
				})?;
				if let Some(failure) = broker_failure(secret, response.headers()) {
					return Err(failure.into_error());
				}
				if let Some(error) = auth::challenge(&response, endpoint) {
					return Err(auth::finish(error));
				}
				if !response.status().is_success() {
					return Err(Error::External(format!(
						"HTTP tool returned {}",
						response.status()
					)));
				}
				crate::response::json(response, 256_000).await
			}
			ToolConfig::Mcp {
				endpoint,
				tool_name,
				idempotency_argument,
				..
			} => {
				let endpoint_client = self.client(endpoint)?;
				let client = if is_brokered(secret) {
					&self.clients.permitted
				} else {
					endpoint_client
				};
				let mut args = input
					.as_object()
					.cloned()
					.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
				if let Some(argument) = idempotency_argument {
					args.insert(argument.clone(), json!(key));
				}
				let mut result = mcp_call(client, endpoint, tool_name, args.clone(), secret).await;
				if let Err(error) = &result
					&& auth::retry(error, secret).await
				{
					result = mcp_call(client, endpoint, tool_name, args, secret).await;
				}
				match result {
					Err(error) => {
						let error = auth::finish(error);
						auth::error_value(&error).ok_or(error)
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
) -> Result<Value> {
	auth::validate(secret, endpoint)?;
	let service = mcp_session(client, endpoint, secret).await?;
	let result = service
		.call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
		.await
		.map_err(|error| mcp::call_error(&error));
	let _ = service.cancel().await;
	mcp::result_value(result?)
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
		Some(CallSecret::Brokered { source, rest, .. }) => {
			let route = source.route().await?;
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
		let service = mcp_session(client, endpoint, None).await?;
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
			.map_err(|e| Error::External(format!("MCP tool listing failed: {e}")))?;
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
