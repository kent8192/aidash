//! Authority RPCs preserve permanent denials, pending visibility and semantic reasons.
use crate::{
	Error, Result,
	ports::federation::authority::{Peers, Reply, Transport},
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
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
		let reply = async {
			let route = self.peers.route(node).await?;
			self.transport.request(&route, path, body).await
		}
		.await
		.map_err(|error| {
			tracing::warn!(%node,%path,error=%error,"authority request failed");
			Error::External("remote execution authority unavailable".into())
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
	if reply.status == 403
		&& let Some(reason) = reply.execution_refusal
	{
		return Err(Error::RemoteExecutionRefused(reason));
	}
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
