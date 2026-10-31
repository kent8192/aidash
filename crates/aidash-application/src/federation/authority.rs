//! Authority RPCs preserve permanent denials, pending visibility and semantic reasons.
use crate::{
	Error, Result,
	ports::federation::authority::{Peers, Reply, Transport},
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
/// Only personal tool calls have an invocation journal that classifies delivery.
pub fn is_connection_call(path: &str, body: &Value) -> bool {
	path == "/scoped/execution/commands"
		&& body.get("operation").and_then(Value::as_str)
			== Some(aidash_domain::identity::commands::connections::CALL)
}

pub struct Client {
	peers: Arc<dyn Peers>,
	transport: Arc<dyn Transport>,
}
impl Client {
	pub fn new(peers: Arc<dyn Peers>, transport: Arc<dyn Transport>) -> Self {
		Self { peers, transport }
	}
	pub async fn request<T: DeserializeOwned>(
		&self,
		node: &str,
		path: &str,
		body: &Value,
	) -> Result<T> {
		let connection_call = is_connection_call(path, body);
		let route = self.peers.route(node).await.map_err(|error| {
			tracing::warn!(%node, %path, %error, "authority route failed");
			if connection_call {
				Error::ToolNotDelivered {
					message: "remote execution authority unavailable".into(),
					retry_after: None,
				}
			} else {
				Error::External("remote execution authority unavailable".into())
			}
		})?;
		let reply = self
			.transport
			.request(&route, path, body)
			.await
			.map_err(|error| {
				tracing::warn!(%node, %path, %error, "authority request failed");
				if connection_call && matches!(error, Error::ToolNotDelivered { .. }) {
					error
				} else {
					Error::External("remote execution authority unavailable".into())
				}
			})?;
		let status = reply.status;
		let value = classify(reply).inspect_err(|error| {
			if matches!(error,Error::External(message) if message=="remote execution authority unavailable")
			{
				tracing::warn!(%node,%path,status,"authority request rejected");
			}
		})?;
		serde_json::from_slice(&value)
			.map_err(|_| Error::External("invalid remote authority response".into()))
	}
}
fn classify(reply: Reply) -> Result<Vec<u8>> {
	if !(200..=299).contains(&reply.status)
		&& let Some(reason) = reply.semantic_reason
	{
		return Err(Error::RemoteSemantic(reason));
	}
	match reply.status {
		200..=299 => reply
			.body
			.map_err(|_| Error::External("invalid remote authority response".into())),
		401 | 403 | 404 => Err(Error::Forbidden),
		409 => Err(Error::Conflict("remote execution authority changed".into())),
		503 if reply.transaction_pending => Err(Error::TransactionPending),
		_ => Err(Error::External(
			"remote execution authority unavailable".into(),
		)),
	}
}
#[cfg(test)]
mod tests;
