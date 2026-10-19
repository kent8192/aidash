//! HTTP and MCP transports for validated application tools.
use aidash_application::execution::bounded_utf8_end;
use aidash_application::{Error, Result, ports::tools::ToolTransport, tools::required};
use aidash_domain::tool::ToolConfig;
use async_trait::async_trait;
use rmcp::{
	ServiceExt,
	model::CallToolRequestParams,
	transport::{
		StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
	},
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
mod mcp;

/// Total budget for a native `http_get`, covering DNS resolution and the request.
const NATIVE_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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

pub struct HttpTools {
	pub client: reqwest::Client,
}
#[async_trait]
impl ToolTransport for HttpTools {
	async fn invoke(
		&self,
		configuration: &ToolConfig,
		input: Value,
		key: &str,
		secret: Option<&SecretString>,
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
				let mut request = self
					.client
					.post(endpoint)
					.header("idempotency-key", key)
					.json(&input);
				// A Tenant Secret goes as the bearer credential, to the host the
				// application layer already checked against its recorded hosts.
				if let Some(secret) = secret {
					request = request.bearer_auth(secret.expose_secret());
				}
				let response = request.send().await.map_err(crate::http_error)?;
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
				let mut transport_config =
					StreamableHttpClientTransportConfig::with_uri(endpoint.clone());
				if let Some(secret) = secret {
					transport_config = transport_config.auth_header(secret.expose_secret());
				}
				let transport = StreamableHttpClientTransport::with_client(
					mcp::BoundedClient(self.client.clone()),
					transport_config,
				);
				let service = ()
					.serve(transport)
					.await
					.map_err(|e| Error::External(format!("MCP initialization failed: {e}")))?;
				let mut args = input
					.as_object()
					.cloned()
					.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
				if let Some(argument) = idempotency_argument {
					args.insert(argument.clone(), json!(key));
				}
				let result = service
					.call_tool(CallToolRequestParams::new(tool_name.clone()).with_arguments(args))
					.await
					.map_err(|e| Error::External(format!("MCP call failed: {e}")));
				let _ = service.cancel().await;
				let result = result?;
				if result.is_error == Some(true) {
					return Ok(json!({"is_error":true,"content":result.content}));
				}
				Ok(serde_json::to_value(result)?)
			}
			_ => Err(Error::Invalid(
				"tool configuration has no external transport".into(),
			)),
		}
	}
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
