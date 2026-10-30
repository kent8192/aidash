//! Bound transport bytes before either SSE framing or JSON-RPC decoding.
use aidash_application::Error;
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
use serde_json::{Value, json};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc};

const LIMIT: usize = 256_000;
#[derive(Clone)]
pub(crate) struct BoundedClient(pub reqwest::Client);
type Failure = StreamableHttpError<reqwest::Error>;
fn invalid(message: &str) -> Failure {
	Failure::UnexpectedServerResponse(message.to_owned().into())
}

/// rmcp wraps a transport failure without exposing it as a `source`, so the
/// resolver's refusal of a non-public address is looked up in the wrapper.
fn refused(error: &DynamicTransportError) -> bool {
	matches!(
		error.error.downcast_ref::<Failure>(),
		Some(Failure::Client(source)) if crate::keyless::refused(source)
	)
}

fn classify_transport(error: &DynamicTransportError) -> Option<Error> {
	match error.error.downcast_ref::<Failure>() {
		Some(Failure::UnexpectedServerResponse(message)) => {
			crate::tools::decode_mcp_http_status(message)
		}
		Some(Failure::Client(source))
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
		ClientInitializeError::TransportError { error: source, .. } => {
			if refused(source) {
				refusal()
			} else {
				classify_transport(source).unwrap_or_else(|| {
					Error::External(format!("MCP initialization failed: {error}"))
				})
			}
		}
		_ => Error::External(format!("MCP initialization failed: {error}")),
	}
}

pub(crate) fn call_error(error: &ServiceError) -> Error {
	match error {
		ServiceError::TransportSend(source) => {
			if refused(source) {
				refusal()
			} else {
				classify_transport(source)
					.unwrap_or_else(|| Error::External(format!("MCP call failed: {error}")))
			}
		}
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
fn events(
	mut response: reqwest::Response,
	request_id: Option<Value>,
) -> BoxStream<'static, Result<Sse, SseError>> {
	// A session is only used for one tool invocation. Cap its entire response,
	// including unterminated SSE fields, before the parser can buffer them.
	let signal = Arc::new(std::sync::Mutex::new(None));
	let signal_writer = signal.clone();
	let expected = request_id.clone();
	let stream = async_stream::try_stream! {
		let mut received = 0_usize;
		let mut prefix = Vec::new();
		while let Some(chunk) = response.chunk().await.map_err(std::io::Error::other)? {
			let remaining = LIMIT.saturating_sub(prefix.len());
			prefix.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
			received = received.saturating_add(chunk.len());
			if received > LIMIT {
				let mut outcome = crate::tools::mcp_sse_signal(&prefix, expected.as_ref());
				if let Some((_, final_result)) = &mut outcome && *final_result {
					*final_result = crate::tools::mcp_sse_complete(&mut response, &chunk[remaining..]).await;
				}
				*signal_writer.lock().expect("SSE signal") = outcome;
				Err(std::io::Error::other("MCP response exceeds 256 KB"))?;
			}
			yield chunk;
		}
	};
	let parsed =
		SseStream::from_bytes_stream(stream.map(|item: Result<_, std::io::Error>| item)).boxed();
	crate::tools::mcp_sse_wrap(parsed, signal)
}
impl StreamableHttpClient for BoundedClient {
	type Error = reqwest::Error;
	async fn post_message(
		&self,
		uri: Arc<str>,
		message: ClientJsonRpcMessage,
		session: Option<Arc<str>>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<StreamableHttpPostResponse, Failure> {
		let request_value = serde_json::to_value(&message).ok();
		let request_id = request_value
			.as_ref()
			.filter(|value| value.get("method").and_then(Value::as_str) == Some("tools/call"))
			.and_then(|value| value.get("id"))
			.cloned();
		let response = request(
			&self.0,
			reqwest::Method::POST,
			&uri,
			session.as_deref(),
			token,
			headers,
		)
		.json(&message)
		.send()
		.await?;
		if matches!(
			response.status(),
			reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
		) {
			return Ok(StreamableHttpPostResponse::Accepted);
		}
		if response.status().is_client_error() || response.status().is_server_error() {
			return Err(invalid(&crate::tools::mcp_http_status(response).await));
		}
		let response = response.error_for_status()?;
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
				events(response, request_id),
				session,
			));
		}
		if !content_type.starts_with("application/json") {
			return Err(invalid("unsupported MCP content type"));
		}
		let value: Value = if request_id.is_some() {
			crate::tools::mcp_json_body(response, request_id.as_ref(), LIMIT)
				.await
				.map_err(|error| invalid(&error))?
		} else {
			crate::response::json(response, LIMIT)
				.await
				.map_err(|error| invalid(&error.to_string()))?
		};
		let final_result = crate::tools::mcp_result_identified(&value, request_id.as_ref());
		let message = serde_json::from_value::<rmcp::model::ServerJsonRpcMessage>(value).map_err(
			|error| {
				if final_result {
					invalid(&crate::tools::mcp_unreadable())
				} else {
					invalid(&error.to_string())
				}
			},
		)?;
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
		self.0.delete_session(uri, session, token, headers).await
	}
	async fn get_stream(
		&self,
		uri: Arc<str>,
		session: Arc<str>,
		last: Option<String>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
		let mut request = request(
			&self.0,
			reqwest::Method::GET,
			&uri,
			Some(&session),
			token,
			headers,
		);
		if let Some(last) = last {
			request = request.header("last-event-id", last);
		}
		let response = request.send().await?;
		if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
			return Err(Failure::ServerDoesNotSupportSse);
		}
		Ok(events(response.error_for_status()?, None))
	}
}

#[cfg(test)]
mod tests;
