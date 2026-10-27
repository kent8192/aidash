//! Bound transport bytes before either SSE framing or JSON-RPC decoding.
use super::broker;
use aidash_application::{
	Error,
	tenant_secrets::{SecretRoute, SecretRouteSource},
};
use futures_util::{StreamExt, stream::BoxStream};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
	model::{CallToolResult, ClientJsonRpcMessage},
	service::{ClientInitializeError, ServiceError},
	transport::{
		DynamicTransportError,
		streamable_http_client::{
			SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
		},
	},
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use sse_stream::{Sse, SseStream};
use std::{
	collections::HashMap,
	fmt,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::time::Instant;

const LIMIT: usize = 256_000;
/// A Tenant Secret Capability Token lives for 60 seconds. One MCP session
/// reuses a token for 50 seconds, then asks the broker for another.
const REFRESH_AFTER: Duration = Duration::from_secs(50);

/// The MCP transport's own failure. A broker refusal keeps the correlation the
/// application shows and the reason code the server logs.
#[derive(Debug)]
pub(super) enum McpError {
	Http(reqwest::Error),
	App(Error),
	Broker(broker::BrokerFailure),
}
impl fmt::Display for McpError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Http(error) => write!(formatter, "{error}"),
			Self::App(error) => write!(formatter, "{error}"),
			Self::Broker(_) => formatter.write_str("Tenant Secret broker call failed"),
		}
	}
}
impl std::error::Error for McpError {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Http(error) => Some(error),
			Self::App(error) => Some(error),
			Self::Broker(_) => None,
		}
	}
}
type Failure = StreamableHttpError<McpError>;
impl From<McpError> for Failure {
	fn from(error: McpError) -> Self {
		Self::Client(error)
	}
}

#[derive(Clone)]
pub(super) struct BoundedClient {
	client: reqwest::Client,
	broker: Option<Brokered>,
}

impl BoundedClient {
	pub(super) fn direct(client: reqwest::Client) -> Self {
		Self {
			client,
			broker: None,
		}
	}
	pub(super) fn brokered(
		client: reqwest::Client,
		source: Arc<dyn SecretRouteSource>,
		rest: String,
		first: SecretRoute,
	) -> Self {
		Self {
			client,
			broker: Some(Brokered::new(source, rest, first)),
		}
	}
	/// The broker's forward URL and token for this request, or the configured
	/// URI and bearer value on a self-hosted node.
	async fn target(
		&self,
		uri: &str,
		bearer: Option<String>,
	) -> Result<(String, Option<String>, Option<SecretString>), Failure> {
		let Some(broker) = &self.broker else {
			return Ok((uri.to_owned(), bearer, None));
		};
		let route = broker.route().await.map_err(Failure::Client)?;
		let url = broker::forward_url(&route.base, &broker.rest)
			.map_err(McpError::App)
			.map_err(Failure::Client)?;
		Ok((url.to_string(), None, Some(route.token)))
	}
	fn send_error(&self, error: reqwest::Error) -> Failure {
		if self.broker.is_some() && !crate::keyless::refused(&error) {
			return Failure::Client(McpError::Broker(broker::BrokerFailure::connect_failure()));
		}
		Failure::Client(McpError::Http(error))
	}
	fn refusal(&self, response: &reqwest::Response) -> Option<Failure> {
		self.broker.as_ref()?;
		broker::BrokerFailure::from_headers(response.headers())
			.map(McpError::Broker)
			.map(Failure::Client)
	}
}

/// A shared token cache for one MCP session: the same token serves every
/// request until 50 seconds have passed.
#[derive(Clone)]
pub(super) struct Brokered {
	source: Arc<dyn SecretRouteSource>,
	rest: String,
	cached: Arc<Mutex<Option<Cached>>>,
}
#[derive(Clone)]
struct Cached {
	route: SecretRoute,
	since: Instant,
}
impl Brokered {
	pub(super) fn new(
		source: Arc<dyn SecretRouteSource>,
		rest: String,
		first: SecretRoute,
	) -> Self {
		Self {
			source,
			rest,
			cached: Arc::new(Mutex::new(Some(Cached {
				route: first,
				since: Instant::now(),
			}))),
		}
	}
	/// The cached route while it is younger than 50 seconds, otherwise a fresh
	/// one from the broker adapter. A failure here never falls back to a direct
	/// call.
	pub(super) async fn route(&self) -> Result<SecretRoute, McpError> {
		let cached = {
			let cache = self.cached.lock().unwrap();
			cache
				.as_ref()
				.filter(|cached| cached.since.elapsed() < REFRESH_AFTER)
				.map(|cached| cached.route.clone())
		};
		if let Some(route) = cached {
			return Ok(route);
		}
		let route = self.source.route().await.map_err(McpError::App)?;
		*self.cached.lock().unwrap() = Some(Cached {
			route: route.clone(),
			since: Instant::now(),
		});
		Ok(route)
	}
}

fn invalid(message: &str) -> Failure {
	Failure::UnexpectedServerResponse(message.to_owned().into())
}

/// rmcp wraps a transport failure without exposing it as a `source`, so the
/// resolver's refusal of a non-public address is looked up in the wrapper.
fn refused(error: &DynamicTransportError) -> bool {
	matches!(
		error.error.downcast_ref::<Failure>(),
		Some(Failure::Client(McpError::Http(source))) if crate::keyless::refused(source)
	)
}

/// The broker or route failure inside an rmcp transport error, rebuilt as the
/// application error its caller sees.
fn broker_error(error: &DynamicTransportError) -> Option<Error> {
	match error.error.downcast_ref::<Failure>() {
		Some(Failure::Client(McpError::Broker(failure))) => Some(failure.clone().into_error()),
		Some(Failure::Client(McpError::App(error))) => Some(repeat(error)),
		_ => None,
	}
}

fn repeat(error: &Error) -> Error {
	match error {
		Error::Invalid(message) => Error::Invalid(message.clone()),
		Error::BrokerUnavailable { correlation } => Error::BrokerUnavailable {
			correlation: correlation.clone(),
		},
		Error::BrokerRejected { correlation } => Error::BrokerRejected {
			correlation: correlation.clone(),
		},
		other => Error::External(other.to_string()),
	}
}

/// A refused destination is an authority failure, never a retried outage.
pub(crate) fn initialization_error(error: &ClientInitializeError) -> Error {
	match error {
		ClientInitializeError::TransportError { error: source, .. } if refused(source) => refusal(),
		ClientInitializeError::TransportError { error: source, .. } => broker_error(source)
			.unwrap_or_else(|| Error::External(format!("MCP initialization failed: {error}"))),
		_ => Error::External(format!("MCP initialization failed: {error}")),
	}
}

pub(crate) fn call_error(error: &ServiceError) -> Error {
	match error {
		ServiceError::TransportSend(source) if refused(source) => refusal(),
		ServiceError::TransportSend(source) => broker_error(source)
			.unwrap_or_else(|| Error::External(format!("MCP call failed: {error}"))),
		_ => Error::External(format!("MCP call failed: {error}")),
	}
}

/// An MCP error result is data for the model; any other result is passed on whole.
pub(crate) fn result_value(result: CallToolResult) -> aidash_application::Result<Value> {
	if result.is_error == Some(true) {
		return Ok(json!({"is_error":true,"content":result.content}));
	}
	Ok(serde_json::to_value(result)?)
}

fn refusal() -> Error {
	tracing::warn!("tool endpoint refused: its host resolves to a non-public address");
	Error::Forbidden
}
fn request(
	client: &reqwest::Client,
	method: reqwest::Method,
	uri: &str,
	session: Option<&str>,
	bearer: Option<String>,
	secret_call: Option<&str>,
	headers: HashMap<HeaderName, HeaderValue>,
) -> reqwest::RequestBuilder {
	let mut request = client
		.request(method, uri)
		.headers(headers.into_iter().collect())
		.header("accept", "application/json, text/event-stream");
	if let Some(session) = session {
		request = request.header("mcp-session-id", session);
	}
	if let Some(token) = bearer {
		request = request.bearer_auth(token);
	}
	if let Some(token) = secret_call {
		request = request.header(broker::SECRET_CALL, token);
	}
	request
}
fn events(mut response: reqwest::Response) -> BoxStream<'static, Result<Sse, SseError>> {
	// A session is only used for one tool invocation. Cap its entire response,
	// including unterminated SSE fields, before the parser can buffer them.
	let stream = async_stream::try_stream! {
		let mut received = 0_usize;
		while let Some(chunk) = response.chunk().await.map_err(std::io::Error::other)? {
			received = received.saturating_add(chunk.len());
			if received > LIMIT { Err(std::io::Error::other("MCP response exceeds 256 KB"))?; }
			yield chunk;
		}
	};
	SseStream::from_bytes_stream(stream.map(|item: Result<_, std::io::Error>| item)).boxed()
}
impl StreamableHttpClient for BoundedClient {
	type Error = McpError;
	async fn post_message(
		&self,
		uri: Arc<str>,
		message: ClientJsonRpcMessage,
		session: Option<Arc<str>>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<StreamableHttpPostResponse, Failure> {
		let (target, bearer, secret_call) = self.target(&uri, token).await?;
		let response = request(
			&self.client,
			reqwest::Method::POST,
			&target,
			session.as_deref(),
			bearer,
			secret_call.as_ref().map(ExposeSecret::expose_secret),
			headers,
		)
		.json(&message)
		.send()
		.await
		.map_err(|error| self.send_error(error))?;
		if let Some(failure) = self.refusal(&response) {
			return Err(failure);
		}
		if response.status() == reqwest::StatusCode::NOT_FOUND && session.is_some() {
			return Err(Failure::SessionExpired);
		}
		if matches!(
			response.status(),
			reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
		) {
			return Ok(StreamableHttpPostResponse::Accepted);
		}
		let response = response
			.error_for_status()
			.map_err(|error| Failure::Client(McpError::Http(error)))?;
		let session = response
			.headers()
			.get("mcp-session-id")
			.and_then(|v| v.to_str().ok())
			.map(str::to_owned);
		let content_type = response
			.headers()
			.get("content-type")
			.and_then(|v| v.to_str().ok())
			.unwrap_or("");
		if content_type.starts_with("text/event-stream") {
			return Ok(StreamableHttpPostResponse::Sse(events(response), session));
		}
		if !content_type.starts_with("application/json") {
			return Err(invalid("unsupported MCP content type"));
		}
		let message = crate::response::json(response, LIMIT)
			.await
			.map_err(|e| invalid(&e.to_string()))?;
		Ok(StreamableHttpPostResponse::Json(message, session))
	}
	async fn delete_session(
		&self,
		uri: Arc<str>,
		session: Arc<str>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<(), Failure> {
		let (target, bearer, secret_call) = self.target(&uri, token).await?;
		let response = request(
			&self.client,
			reqwest::Method::DELETE,
			&target,
			Some(&session),
			bearer,
			secret_call.as_ref().map(ExposeSecret::expose_secret),
			headers,
		)
		.send()
		.await
		.map_err(|error| self.send_error(error))?;
		if let Some(failure) = self.refusal(&response) {
			return Err(failure);
		}
		if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
			return Err(Failure::ServerDoesNotSupportDeleteSession);
		}
		response
			.error_for_status()
			.map_err(|error| Failure::Client(McpError::Http(error)))?;
		Ok(())
	}
	async fn get_stream(
		&self,
		uri: Arc<str>,
		session: Arc<str>,
		last: Option<String>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
		let (target, bearer, secret_call) = self.target(&uri, token).await?;
		let mut request = request(
			&self.client,
			reqwest::Method::GET,
			&target,
			Some(&session),
			bearer,
			secret_call.as_ref().map(ExposeSecret::expose_secret),
			headers,
		);
		if let Some(last) = last {
			request = request.header("last-event-id", last);
		}
		let response = request
			.send()
			.await
			.map_err(|error| self.send_error(error))?;
		if let Some(failure) = self.refusal(&response) {
			return Err(failure);
		}
		if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
			return Err(Failure::ServerDoesNotSupportSse);
		}
		Ok(events(
			response
				.error_for_status()
				.map_err(|error| Failure::Client(McpError::Http(error)))?,
		))
	}
}

#[cfg(test)]
mod tests;
