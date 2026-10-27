//! Bound transport bytes before either SSE framing or JSON-RPC decoding.
use aidash_application::{
	Error,
	tenant_secrets::{BearerSource, CallSecret},
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
#[derive(Clone)]
pub(crate) struct BoundedClient(
	pub reqwest::Client,
	pub Option<Arc<dyn BearerSource>>,
	pub Arc<crate::tools::auth::CallProgress>,
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum WireError {
	#[error(transparent)]
	Http(#[from] reqwest::Error),
	#[error(transparent)]
	App(#[from] Error),
}
pub(crate) type Failure = StreamableHttpError<WireError>;
impl BoundedClient {
	pub(crate) fn new(
		client: reqwest::Client,
		auth: Option<&CallSecret>,
		progress: Arc<crate::tools::auth::CallProgress>,
	) -> Self {
		Self(
			client,
			match auth {
				Some(CallSecret::Connection { source, .. }) => Some(source.clone()),
				_ => None,
			},
			progress,
		)
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
fn send_error(error: reqwest::Error) -> Failure {
	Failure::Client(WireError::Http(error))
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

/// A refused destination is an authority failure, never a retried outage.
pub(crate) fn initialization_error(error: &ClientInitializeError) -> Error {
	match error {
		ClientInitializeError::TransportError { error: source, .. } if refused(source) => refusal(),
		ClientInitializeError::TransportError { error: source, .. } => auth_error(source)
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
		ServiceError::TransportSend(source) => auth_error(source)
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
fn events(mut response: reqwest::Response) -> BoxStream<'static, Result<Sse, SseError>> {
	// The GET stream can outlive many calls. Bound each event before the parser
	// buffers its fields, and feed completed events separately so an HTTP chunk
	// containing many events cannot build an unbounded parser queue.
	let stream = async_stream::try_stream! {
		let mut received = 0_usize;
		let mut line_empty = true;
		let mut comment_bytes = None;
		let mut skip_lf = false;
		let mut count_lf = false;
		let mut bom_prefix = Some(0_usize);
		while let Some(chunk) = response.chunk().await.map_err(std::io::Error::other)? {
			let mut start = 0;
			for (offset, byte) in chunk.iter().enumerate() {
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
						if received > LIMIT { Err(std::io::Error::other("MCP SSE event exceeds 256 KB"))?; }
					}
					continue;
				}
				if line_empty && *byte == b':' { comment_bytes = Some(0_usize); }
				// Comments retain no event content after their line ends. Bound their
				// unfinished line separately without resetting an event's existing fields.
				let bytes = comment_bytes.as_mut().unwrap_or(&mut received);
				*bytes += 1;
				if *bytes > LIMIT { Err(std::io::Error::other("MCP SSE event exceeds 256 KB"))?; }
				if matches!(*byte, b'\r' | b'\n') {
					let comment = comment_bytes.take().is_some();
					if line_empty {
						received = 0;
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
	SseStream::from_bytes_stream(stream.map(|item: Result<_, std::io::Error>| item)).boxed()
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
		let request = request(
			&self.0,
			reqwest::Method::POST,
			&uri,
			session.as_deref(),
			self.bearer(token, &uri).await?,
			headers,
		)
		.json(&message);
		if tool_request {
			self.2.sent();
		}
		let response = request.send().await.map_err(send_error)?;
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

		if response.status() == reqwest::StatusCode::NOT_FOUND && session.is_some() {
			return Err(Failure::SessionExpired);
		}
		if matches!(
			response.status(),
			reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
		) {
			return Ok(StreamableHttpPostResponse::Accepted);
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
		// DELETE never decodes a response body.
		let response = request(
			&self.0,
			reqwest::Method::DELETE,
			&uri,
			Some(&session),
			self.bearer(token, &uri).await?,
			headers,
		)
		.send()
		.await
		.map_err(send_error)?;
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
		let mut request = request(
			&self.0,
			reqwest::Method::GET,
			&uri,
			session.as_deref(),
			self.bearer(token, &uri).await?,
			headers,
		);
		if let Some(last) = last {
			request = request.header("last-event-id", last);
		}
		let response = request.send().await.map_err(send_error)?;
		if let Some(error) = self.2.challenge(&response, &uri, false) {
			return Err(Failure::Client(WireError::App(error)));
		}
		if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
			return Err(Failure::ServerDoesNotSupportSse);
		}
		Ok(events(response.error_for_status().map_err(send_error)?))
	}
}

#[cfg(test)]
mod tests;
