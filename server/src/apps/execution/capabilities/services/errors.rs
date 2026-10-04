use serde_json::{Value, json};

impl CapabilityError {
	pub(crate) fn message(status: u16, message: &str) -> Self {
		aidash_domain::capabilities::errors::CapabilityError::message(status, message).into()
	}
	pub(crate) fn stored(value: &Value) -> Option<Self> {
		aidash_domain::capabilities::errors::CapabilityError::stored(value).map(Into::into)
	}
}
impl From<aidash_domain::capabilities::errors::CapabilityError> for CapabilityError {
	fn from(v: aidash_domain::capabilities::errors::CapabilityError) -> Self {
		Self {
			code: v.code,
			message: v.message,
			retryable: v.retryable,
			details: v.details,
		}
	}
}
/// Apply the same safe shape to deserialization, authorization and handler
/// errors without changing the contracts of legacy routes.

#[derive(Clone, Copy)]
pub(crate) struct CapabilityErrors;
#[async_trait::async_trait]
impl reinhardt::http::Middleware for CapabilityErrors {
	async fn process(
		&self,
		request: reinhardt::Request,
		next: std::sync::Arc<dyn reinhardt::http::Handler>,
	) -> reinhardt::http::ViewResult<reinhardt::Response> {
		let mut response = next.handle(request).await?;
		if response.status.is_client_error() || response.status.is_server_error() {
			let value: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
			let text = String::from_utf8_lossy(&response.body);
			let error = CapabilityError::message(
				response.status.as_u16(),
				value["error"].as_str().unwrap_or(&text),
			);
			response.body = serde_json::to_vec(&json!({"error":error}))?.into();
			response.headers.insert(
				http::header::CONTENT_TYPE,
				http::HeaderValue::from_static("application/json"),
			);
		}
		Ok(response)
	}
}

pub use crate::apps::execution::capabilities::serializers::errors::CapabilityError;
