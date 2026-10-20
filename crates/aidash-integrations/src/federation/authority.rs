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
		let semantic_reason = response
			.headers()
			.get("x-aidash-semantic-reason")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| serde_json::from_value(json!(value)).ok());
		let transaction_pending = response
			.headers()
			.get("x-aidash-transaction-pending")
			.is_some_and(|value| value == "1");
		let body = if status.is_success() {
			crate::response::bytes(response, 4_194_304).await
		} else {
			Ok(Vec::new())
		};
		Ok(Reply {
			status: status.as_u16(),
			semantic_reason,
			transaction_pending,
			body,
		})
	}
}
