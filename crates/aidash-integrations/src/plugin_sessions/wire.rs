//! Bound transport bytes before either SSE framing or JSON-RPC decoding.
use aidash_application::{
	Error,
	tenant_secrets::{BearerSource, CallSecret, SecretRouteSource},
};
use futures_util::{StreamExt, stream::BoxStream};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp_modern::{
	model::{CallToolResult, ClientJsonRpcMessage},
	service::{ClientInitializeError, ServiceError},
	transport::{
		DynamicTransportError,
		streamable_http_client::{
			SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
		},
	},
};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc};

const LIMIT: usize = 256_000;
/// The transport of one plugin session: the HTTP client, a person's OAuth bearer source, the
/// call's progress, a broker route source (an OAuth Connection or a Tenant Secret forwarded by the
/// Credential Broker) and a self-hosted pasted secret already rendered as its header.
#[derive(Clone)]
pub(crate) struct BoundedClient(
	pub reqwest::Client,
	pub Option<Arc<dyn BearerSource>>,
	pub Arc<crate::tools::auth::CallProgress>,
	pub Option<(Arc<dyn SecretRouteSource>, String)>,
	pub Option<(HeaderName, HeaderValue)>,
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum WireError {
	#[error(transparent)]
	Http(#[from] reqwest::Error),
	#[error(transparent)]
	App(#[from] Error),
	#[error("Tenant Secret broker call failed")]
	Broker(crate::broker::BrokerFailure),
}
pub(crate) type Failure = StreamableHttpError<WireError>;
impl BoundedClient {
	/// A self-hosted pasted secret whose value cannot be a header value is refused before any I/O,
	/// so the server never receives the call without the credential it was configured with.
	pub(crate) fn new(
		client: reqwest::Client,
		auth: Option<&CallSecret>,
		progress: Arc<crate::tools::auth::CallProgress>,
	) -> aidash_application::Result<Self> {
		Ok(Self(
			client,
			match auth {
				Some(CallSecret::Connection { source, .. }) => Some(source.clone()),
				_ => None,
			},
			progress,
			match auth {
				Some(
					CallSecret::BrokeredConnection { source, rest, .. }
					| CallSecret::Brokered { source, rest, .. },
				) => Some((source.clone(), rest.clone())),
				_ => None,
			},
			match auth {
				Some(CallSecret::Value { value, attach, .. }) => {
					Some(secret_header(value, attach)?)
				}
				_ => None,
			},
		))
	}
	async fn request(
		&self,
		method: reqwest::Method,
		uri: &str,
		session: Option<&str>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<reqwest::RequestBuilder, Failure> {
		if let Some((source, rest)) = &self.3 {
			let route = source
				.route()
				.await
				.map_err(|e| Failure::Client(WireError::App(self.2.source_error(e, uri))))?;
			let url = crate::broker::forward_url(&route.base, rest)
				.map_err(|e| Failure::Client(WireError::App(e)))?;
			let mut token = HeaderValue::from_str(route.token.expose_secret())
				.map_err(|_| invalid("invalid OAuth broker token"))?;
			token.set_sensitive(true);
			return Ok(
				request(&self.0, method, url.as_str(), session, None, headers)
					.header(crate::broker::SECRET_CALL, token),
			);
		}
		let request = request(
			&self.0,
			method,
			uri,
			session,
			self.bearer(token, uri).await?,
			headers,
		);
		Ok(match &self.4 {
			Some((name, value)) => request.header(name.clone(), value.clone()),
			None => request,
		})
	}
	fn send_error(&self, error: reqwest::Error) -> Failure {
		if self.3.is_some() && !crate::keyless::refused(&error) {
			return Failure::Client(WireError::Broker(
				crate::broker::BrokerFailure::connect_failure(),
			));
		}
		send_error(error)
	}

	fn broker_error(&self, response: &reqwest::Response) -> Option<Failure> {
		self.3.as_ref()?;
		crate::broker::BrokerFailure::from_headers(response.headers())
			.map(|e| Failure::Client(WireError::Broker(e)))
	}

	async fn bearer(
		&self,
		token: Option<String>,
		endpoint: &str,
	) -> Result<Option<String>, Failure> {
		match &self.1 {
			Some(source) => Ok(Some(
				source
					.bearer()
					.await
					.map_err(|e| Failure::Client(WireError::App(self.2.source_error(e, endpoint))))?
					.expose_secret()
					.to_owned(),
			)),
			None => Ok(token),
		}
	}
}
/// `Authorization: Bearer <value>`, or the raw value in any other header, as the broker attaches
/// it from the same call (`aidash_capability::tenant_secret::Attach`).
fn secret_header(
	value: &secrecy::SecretString,
	attach: &aidash_capability::tenant_secret::Attach,
) -> aidash_application::Result<(HeaderName, HeaderValue)> {
	use aidash_capability::tenant_secret::Attach;
	let unavailable = || Error::Invalid(aidash_domain::tenant_secrets::UNAVAILABLE.into());
	let (name, rendered) = match attach {
		Attach::Bearer => (
			reqwest::header::AUTHORIZATION,
			format!("Bearer {}", value.expose_secret()),
		),
		Attach::Header { name } => (
			HeaderName::from_bytes(name.as_bytes()).map_err(|_| unavailable())?,
			value.expose_secret().to_owned(),
		),
	};
	let mut rendered = HeaderValue::from_str(&rendered).map_err(|_| unavailable())?;
	rendered.set_sensitive(true);
	Ok((name, rendered))
}

fn send_error(error: reqwest::Error) -> Failure {
	Failure::Client(WireError::Http(error))
}
fn broker_failure(error: &DynamicTransportError) -> Option<Error> {
	match error.error.downcast_ref::<Failure>() {
		Some(Failure::Client(WireError::Broker(f))) => Some(f.clone().into_error()),
		_ => None,
	}
}

fn auth_error(error: &DynamicTransportError) -> Option<Error> {
	match error.error.downcast_ref::<Failure>() {
		Some(Failure::Client(WireError::App(error))) => crate::tools::auth::repeat(error),
		_ => None,
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
		Some(Failure::Client(WireError::Http(source))) if crate::keyless::refused(source)
	)
}

fn classify_transport(error: &DynamicTransportError) -> Option<Error> {
	match error.error.downcast_ref::<Failure>() {
		Some(Failure::UnexpectedServerResponse(message)) => {
			crate::tools::decode_mcp_http_status(message)
		}
		Some(Failure::Client(WireError::Http(source)))
			if source.is_connect()
				|| matches!(source.status().map(|s| s.as_u16()), Some(408 | 429)) =>
		{
			Some(Error::ToolNotDelivered {
				message: "MCP endpoint could not accept the request".into(),
				retry_after: None,
			})
		}
		_ => None,
	}
}

/// A refused destination stays distinguishable until the call adapter returns it as data.
pub(crate) fn initialization_error(error: &ClientInitializeError) -> Error {
	match error {
		ClientInitializeError::TransportError { error: source, .. } if refused(source) => refusal(),
		ClientInitializeError::TransportError { error: source, .. } => broker_failure(source)
			.or_else(|| auth_error(source))
			.or_else(|| classify_transport(source))
			.unwrap_or_else(|| Error::External(format!("MCP initialization failed: {error}"))),
		ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
			initialization_error(fallback)
		}
		_ => Error::External(format!("MCP initialization failed: {error}")),
	}
}

pub(crate) fn call_error(error: &ServiceError) -> Error {
	match error {
		ServiceError::TransportSend(source) if refused(source) => refusal(),
		ServiceError::TransportSend(source) => broker_failure(source)
			.or_else(|| auth_error(source))
			.or_else(|| classify_transport(source))
			.unwrap_or_else(|| Error::External(format!("MCP call failed: {error}"))),
		_ => Error::External(format!("MCP call failed: {error}")),
	}
}

pub(crate) fn listing_error(error: &ServiceError) -> Error {
	if let ServiceError::McpError(error) = error
		&& crate::tools::mcp_sse_outcome(&error.message).is_some()
	{
		return aidash_application::ports::tools::ListFailure::ListingTooLarge.into();
	}
	call_error(error)
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
	token: Option<String>,
	headers: HashMap<HeaderName, HeaderValue>,
) -> reqwest::RequestBuilder {
	let mut request = client
		.request(method, uri)
		.headers(headers.into_iter().collect())
		.header("accept", "application/json, text/event-stream");
	if let Some(session) = session {
		request = request.header("mcp-session-id", session);
	}
	if let Some(token) = token {
		request = request.bearer_auth(token);
	}
	request
}
fn events(
	mut response: reqwest::Response,
	request_id: Option<Value>,
	listing: bool,
) -> BoxStream<'static, Result<Sse, SseError>> {
	// The GET stream can outlive many calls. Bound each event before the parser
	// buffers its fields, and feed completed events separately so an HTTP chunk
	// containing many events cannot build an unbounded parser queue.
	let limit = if listing {
		aidash_application::mcp_listing::MAX_LISTED_SCHEMA_BYTES + LIMIT
	} else {
		LIMIT
	};
	let oversized = if listing {
		"MCP tool listing exceeds 1 MiB"
	} else {
		"MCP SSE event exceeds 256 KB"
	};
	let signal = Arc::new(std::sync::Mutex::new(None));
	let signal_writer = signal.clone();
	let expected = request_id.clone();
	let stream = async_stream::try_stream! {
		let mut received = 0_usize;
		let mut prefix = Vec::new();
		let mut line_empty = true;
		let mut comment_bytes = None;
		let mut skip_lf = false;
		let mut count_lf = false;
		let mut bom_prefix = Some(0_usize);
		while let Some(chunk) = response.chunk().await.map_err(std::io::Error::other)? {
			let mut start = 0;
			for (offset, byte) in chunk.iter().enumerate() {
				if prefix.len() < limit { prefix.push(*byte); }
				// sse-stream ignores one initial UTF-8 BOM, including before a comment.
				if let Some(matched) = bom_prefix {
					if *byte == b"\xef\xbb\xbf"[matched] {
						bom_prefix = (matched < 2).then_some(matched + 1);
						continue;
					}
					bom_prefix = None;
					// A partial prefix is ordinary line content when it is not a BOM.
					received += matched;
					line_empty = matched == 0;
				}
				// Match sse-stream's CR, LF and CRLF framing across chunks. An LF
				// after a comment or an event-ending CR adds no retained event bytes.
				if std::mem::take(&mut skip_lf) && *byte == b'\n' {
					if count_lf {
						received += 1;
						if received > limit {
							let mut outcome = crate::tools::mcp_sse_signal(&prefix, expected.as_ref());
							if let Some((_, final_result)) = &mut outcome { *final_result = false; }
							*signal_writer.lock().expect("SSE signal") = outcome;
							Err(std::io::Error::other(oversized))?;
						}
					}
					continue;
				}
				if line_empty && *byte == b':' { comment_bytes = Some(0_usize); }
				// Comments retain no event content after their line ends. Bound their
				// unfinished line separately without resetting an event's existing fields.
				let bytes = comment_bytes.as_mut().unwrap_or(&mut received);
				*bytes += 1;
				if *bytes > limit {
					let mut outcome = crate::tools::mcp_sse_signal(&prefix, expected.as_ref());
					if let Some((_, final_result)) = &mut outcome && *final_result && !listing {
						*final_result = if matches!(*byte, b'\r' | b'\n') { false } else { crate::tools::mcp_sse_complete(&mut response, &chunk[offset + 1..]).await };
					}
					*signal_writer.lock().expect("SSE signal") = outcome;
					Err(std::io::Error::other(oversized))?;
				}
				if matches!(*byte, b'\r' | b'\n') {
					let comment = comment_bytes.take().is_some();
					if line_empty {
						received = 0;
						prefix.clear();
						yield chunk.slice(start..offset + 1);
						start = offset + 1;
					}
					count_lf = !comment && !line_empty;
					line_empty = true;
					skip_lf = *byte == b'\r';
				} else {
					line_empty = false;
				}
			}
			if start < chunk.len() { yield chunk.slice(start..); }
		}
	};
	let parsed =
		SseStream::from_bytes_stream(stream.map(|item: Result<_, std::io::Error>| item)).boxed();
	crate::tools::mcp_sse_wrap(parsed, signal)
}
impl StreamableHttpClient for BoundedClient {
	type Error = WireError;
	async fn post_message(
		&self,
		uri: Arc<str>,
		message: ClientJsonRpcMessage,
		session: Option<Arc<str>>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<StreamableHttpPostResponse, Failure> {
		let tool_request = matches!(&message, ClientJsonRpcMessage::Request(request)
			if matches!(request.request, rmcp_modern::model::ClientRequest::CallToolRequest(_)));
		let discover_id = match &message {
			ClientJsonRpcMessage::Request(request)
				if matches!(
					request.request,
					rmcp_modern::model::ClientRequest::DiscoverRequest(_)
				) =>
			{
				Some(request.id.clone())
			}
			_ => None,
		};
		let request_value = serde_json::to_value(&message).ok();
		let listing = request_value
			.as_ref()
			.and_then(|value| value["method"].as_str())
			== Some("tools/list");
		let request_id = request_value
			.as_ref()
			.filter(|value| value.get("method").and_then(Value::as_str) == Some("tools/call"))
			.and_then(|value| value.get("id"))
			.cloned();
		let request = self
			.request(
				reqwest::Method::POST,
				&uri,
				session.as_deref(),
				token,
				headers,
			)
			.await?
			.json(&message);
		if tool_request {
			self.2.sent();
		}
		let response = request.send().await.map_err(|e| self.send_error(e))?;
		if let Some(error) = self.broker_error(&response) {
			return Err(error);
		}
		if let Some(error) = self.2.challenge(&response, &uri, tool_request) {
			return Err(Failure::Client(WireError::App(error)));
		}
		let status = response.status();
		if let Some(id) = discover_id
			&& session.is_none()
			&& status.is_client_error()
			&& !matches!(
				status,
				reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
			) {
			let body = crate::response::bytes(response, LIMIT)
				.await
				.map_err(|_| invalid("MCP discovery rejection exceeds limits"))?;
			let error =
				match serde_json::from_slice::<rmcp_modern::model::ServerJsonRpcMessage>(&body) {
					Ok(rmcp_modern::model::ServerJsonRpcMessage::Error(error)) => error.error,
					_ => {
						rmcp_modern::ErrorData::invalid_request("legacy discovery rejection", None)
					}
				};
			return Ok(StreamableHttpPostResponse::Json(
				rmcp_modern::model::ServerJsonRpcMessage::error(error, Some(id)),
				None,
			));
		}

		if matches!(
			response.status(),
			reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
		) {
			return Ok(StreamableHttpPostResponse::Accepted);
		}
		if response.status().is_client_error() || response.status().is_server_error() {
			return Err(invalid(&crate::tools::mcp_http_status(response).await));
		}
		let response = response.error_for_status().map_err(send_error)?;
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
			return Ok(StreamableHttpPostResponse::Sse(
				events(
					response,
					if listing {
						request_value
							.as_ref()
							.and_then(|value| value.get("id"))
							.cloned()
					} else {
						request_id
					},
					listing,
				),
				session,
			));
		}
		if !content_type.starts_with("application/json") {
			return Err(invalid("unsupported MCP content type"));
		}
		let value: Value = if listing {
			crate::tools::mcp_listing_json(response, LIMIT).await.map_err(|error| {
				if matches!(&error, Error::Port(source) if source.is::<aidash_application::ports::tools::ListFailure>()) {
					Failure::Client(WireError::App(error))
				} else {
					invalid(&error.to_string())
				}
			})?
		} else if request_id.is_some() {
			crate::tools::mcp_json_body(response, request_id.as_ref(), LIMIT)
				.await
				.map_err(|error| invalid(&error))?
		} else {
			crate::response::json(response, LIMIT)
				.await
				.map_err(|error| invalid(&error.to_string()))?
		};
		let final_result = crate::tools::mcp_result_identified(&value, request_id.as_ref());
		let message = serde_json::from_value::<rmcp_modern::model::ServerJsonRpcMessage>(value)
			.map_err(|error| {
				if final_result {
					invalid(&crate::tools::mcp_unreadable())
				} else {
					invalid(&error.to_string())
				}
			})?;
		Ok(StreamableHttpPostResponse::Json(message, session))
	}
	async fn delete_session(
		&self,
		uri: Arc<str>,
		session: Arc<str>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<(), Failure> {
		// DELETE never decodes a response body.
		let response = self
			.request(
				reqwest::Method::DELETE,
				&uri,
				Some(&session),
				token,
				headers,
			)
			.await?
			.send()
			.await
			.map_err(|e| self.send_error(e))?;
		if let Some(error) = self.broker_error(&response) {
			return Err(error);
		}
		if let Some(error) = self.2.challenge(&response, &uri, false) {
			return Err(Failure::Client(WireError::App(error)));
		}
		if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
			return Err(Failure::ServerDoesNotSupportDeleteSession);
		}
		response.error_for_status().map_err(send_error)?;
		Ok(())
	}
	async fn get_stream(
		&self,
		uri: Arc<str>,
		session: Option<Arc<str>>,
		last: Option<String>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
		let mut request = self
			.request(
				reqwest::Method::GET,
				&uri,
				session.as_deref(),
				token,
				headers,
			)
			.await?;
		if let Some(last) = last {
			request = request.header("last-event-id", last);
		}
		let response = request.send().await.map_err(|e| self.send_error(e))?;
		if let Some(error) = self.broker_error(&response) {
			return Err(error);
		}
		if let Some(error) = self.2.challenge(&response, &uri, false) {
			return Err(Failure::Client(WireError::App(error)));
		}
		if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
			return Err(Failure::ServerDoesNotSupportSse);
		}
		Ok(events(
			response.error_for_status().map_err(send_error)?,
			None,
			false,
		))
	}
}

#[cfg(test)]
mod tests;
