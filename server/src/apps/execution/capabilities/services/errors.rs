use serde_json::{Value, json};

impl CapabilityError {
	pub(crate) fn message(status: u16, message: &str) -> Self {
		aidash_domain::capabilities::errors::CapabilityError::message(status, message).into()
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
			let mut error = CapabilityError::message(
				response.status.as_u16(),
				value["error"].as_str().unwrap_or(&text),
			);
			if value["code"]
				== aidash_application::ports::capabilities::execution_pod::RunnerTimeout::CODE
			{
				error.code =
					aidash_application::ports::capabilities::execution_pod::RunnerTimeout::CODE
						.into();
			}
			// A decision that came too late says who decided first and how
			// (ADR 0135).
			if value["code"] == "approval_already_decided" {
				error.details = Some(json!({
					"decided_by": value["decided_by"],
					"outcome": value["outcome"],
				}));
			}
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

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_application::ports::capabilities::execution_pod::RunnerTimeout;
	use reinhardt::http::{Handler, Middleware, ViewResult};
	use reinhardt::{Request, Response};
	use std::sync::Arc;

	struct NoRunnerReply;

	#[async_trait::async_trait]
	impl Handler for NoRunnerReply {
		async fn handle(&self, _request: Request) -> ViewResult<Response> {
			let error = aidash_application::Error::Port(Box::new(RunnerTimeout {
				route: "/v2/previews/start".into(),
				seconds: 75,
			}));
			Ok(crate::Error::from(error).http_response())
		}
	}

	#[tokio::test]
	async fn capability_error_preserves_runner_timeout_code_and_details() {
		let request = Request::builder()
			.uri("/api/core/previews")
			.build()
			.unwrap();
		let response = CapabilityErrors
			.process(request, Arc::new(NoRunnerReply))
			.await
			.unwrap();
		assert_eq!(response.status.as_u16(), 504);
		let body: Value = serde_json::from_slice(&response.body).unwrap();
		assert_eq!(body["error"]["code"], "runner_timeout");
		assert_eq!(
			body["error"]["message"],
			"Runner route /v2/previews/start did not answer within 75 seconds"
		);
		assert_eq!(body["error"]["retryable"], false);
	}
}
