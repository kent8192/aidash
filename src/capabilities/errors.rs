use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CapabilityError {
	pub code: String,
	pub message: String,
	pub retryable: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub details: Option<Value>,
}
fn bounded(value: &str) -> String {
	value.chars().take(2048).collect()
}
impl CapabilityError {
	pub(crate) fn message(status: u16, message: &str) -> Self {
		let prefix = message.split([':', ' ']).next().unwrap_or_default();
		let code = if !prefix.is_empty()
			&& prefix
				.bytes()
				.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
		{
			prefix
		} else {
			match status {
				400 | 413 | 415 | 422 => "INVALID_INPUT",
				401 => "UNAUTHORIZED",
				403 => "FORBIDDEN",
				404 => "NOT_FOUND",
				409 => "CONFLICT",
				429 => "RATE_LIMITED",
				_ => "CAPABILITY_UNAVAILABLE",
			}
		};
		Self {
			code: code.into(),
			message: bounded(message),
			retryable: matches!(status, 429 | 503),
			details: None,
		}
	}
	pub(crate) fn stored(value: &Value) -> Option<Self> {
		if value.is_null() {
			return None;
		}
		if let Ok(mut error) = serde_json::from_value::<Self>(value.clone()) {
			error.message = bounded(&error.message);
			return Some(error);
		}
		if let Some(kind) = value["type"].as_str() {
			return Some(Self {
				code: "PYTHON_ERROR".into(),
				message: bounded(value["message"].as_str().unwrap_or("Python failed")),
				retryable: false,
				details: Some(json!({"type":bounded(kind)})),
			});
		}
		if let Some(code) = value["code"].as_str() {
			return Some(Self {
				code: bounded(code),
				message: bounded(value["message"].as_str().unwrap_or("Execution stopped")),
				retryable: false,
				details: None,
			});
		}
		let message = value
			.as_str()
			.unwrap_or("Execution could not be verified; inspect the operation before retrying.");
		Some(Self::message(409, message))
	}
}
/// Apply the same safe shape to deserialization, authorization and handler
/// errors without changing the contracts of legacy routes.
pub(crate) async fn http(
	request: axum::extract::Request,
	next: axum::middleware::Next,
) -> axum::response::Response {
	use axum::response::IntoResponse;
	let response = next.run(request).await;
	if !response.status().is_client_error() && !response.status().is_server_error() {
		return response;
	}
	let (parts, body) = response.into_parts();
	let bytes = axum::body::to_bytes(body, 16384).await.unwrap_or_default();
	let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
	let text = String::from_utf8_lossy(&bytes);
	let error = CapabilityError::message(
		parts.status.as_u16(),
		value["error"].as_str().unwrap_or(&text),
	);
	let mut response = (parts.status, axum::Json(json!({"error":error}))).into_response();
	for name in ["retry-after", "cache-control"] {
		if let Some(value) = parts.headers.get(name) {
			response
				.headers_mut()
				.insert(axum::http::HeaderName::from_static(name), value.clone());
		}
	}
	response
}
