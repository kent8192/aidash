//! Bound transport bytes before either SSE framing or JSON-RPC decoding.
use futures_util::{StreamExt, stream::BoxStream};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
	model::ClientJsonRpcMessage,
	transport::streamable_http_client::{
		SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
	},
};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc};

const LIMIT: usize = 256_000;
#[derive(Clone)]
pub(super) struct BoundedClient(pub reqwest::Client);
type Failure = StreamableHttpError<reqwest::Error>;
fn invalid(message: &str) -> Failure {
	Failure::UnexpectedServerResponse(message.to_owned().into())
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
	type Error = reqwest::Error;
	async fn post_message(
		&self,
		uri: Arc<str>,
		message: ClientJsonRpcMessage,
		session: Option<Arc<str>>,
		token: Option<String>,
		headers: HashMap<HeaderName, HeaderValue>,
	) -> Result<StreamableHttpPostResponse, Failure> {
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
		if response.status() == reqwest::StatusCode::NOT_FOUND && session.is_some() {
			return Err(Failure::SessionExpired);
		}
		if matches!(
			response.status(),
			reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
		) {
			return Ok(StreamableHttpPostResponse::Accepted);
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
		Ok(events(response.error_for_status()?))
	}
}

#[cfg(test)]
mod tests;
