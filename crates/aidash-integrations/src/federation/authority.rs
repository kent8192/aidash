//! Decode bounded authority payloads and protocol headers while application classifies their meaning.
use super::PeerHttp;
use aidash_application::{
	Result,
	ports::federation::authority::{Reply, Transport},
};
use aidash_domain::federation::Peer;
use async_trait::async_trait;
use serde_json::{Value, json};
#[async_trait]
impl Transport for PeerHttp {
	async fn request(&self, peer: &Peer, path: &str, body: &Value) -> Result<Reply> {
		let response = self.send(peer, "POST", path, Some(body)).await?;
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
