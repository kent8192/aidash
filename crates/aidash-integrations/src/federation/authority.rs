//! Decode bounded authority payloads and protocol headers while application classifies their meaning.
use super::HttpTransport;
use aidash_application::{
	Result,
	ports::federation::authority::{Reply, Transport},
};
use aidash_domain::federation::route::Route;
use async_trait::async_trait;
use serde_json::{Value, json};
#[async_trait]
impl Transport for HttpTransport {
	async fn request(&self, route: &Route, path: &str, body: &Value) -> Result<Reply> {
		let response = self.send(route, "POST", path, Some(body)).await?;
		let status = response.status();
		let execution_refusal = response
			.headers()
			.get("x-aidash-execution-refusal")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| serde_json::from_value(json!(value)).ok());
		let semantic_headers = ["x-aidash-semantic-cause", "x-aidash-semantic-reason"];
		let semantic_reason = semantic_headers
			.iter()
			.filter_map(|name| response.headers().get(*name))
			.filter_map(|value| value.to_str().ok())
			.find_map(|value| serde_json::from_value(json!(value)).ok())
			.or_else(|| {
				semantic_headers
					.iter()
					.any(|name| response.headers().contains_key(*name))
					.then_some(aidash_domain::semantic::Failure::ContextBudget)
			});
		let transaction_pending = response
			.headers()
			.get("x-aidash-transaction-pending")
			.is_some_and(|value| value == "1");
		let mut unreachable = None;
		let policy_busy = response
			.headers()
			.get("x-aidash-policy-busy")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| value.parse().ok());
		let body = if status.is_success() {
			let limit = if path == "/thread-attachment/1/fetch" {
				12 * 1024 * 1024
			} else {
				4_194_304
			};
			crate::response::bytes(response, limit).await
		} else {
			if status.as_u16() == 503
				&& !transaction_pending
				&& let Ok(bytes) = crate::response::bytes(response, 4096).await
			{
				unreachable = serde_json::from_slice::<Value>(&bytes)
					.ok()
					.as_ref()
					.and_then(aidash_application::ports::federation::authority::Unreachable::of);
			}
			Ok(Vec::new())
		};
		Ok(Reply {
			status: status.as_u16(),
			semantic_reason,
			execution_refusal,
			transaction_pending,
			unreachable,
			policy_busy,
			body,
		})
	}
}
