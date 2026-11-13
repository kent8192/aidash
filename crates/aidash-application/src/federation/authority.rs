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
		let bytes = serde_json::to_vec(body)?;
		let request = super::inbound::RequestDigest {
			method: "POST",
			path,
			body: &bytes,
		};
		let route = self.peers.route(node, &request).await.map_err(|error| {
			if matches!(error, Error::Forbidden | Error::Unauthorized) {
				return Error::Forbidden;
			}
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
				if matches!(error, Error::Forbidden | Error::Unauthorized) {
					return Error::Forbidden;
				}
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
	// Another Node could not be reached from the peer: the call is retried or
	// paused like a local one, never reported as an unavailable authority (#770).
	if reply.status == 503
		&& !reply.transaction_pending
		&& let Some(found) = reply.unreachable
	{
		return Err(Error::NodeUnreachable {
			node: found.node,
			after_effect: found.after_effect,
		});
	}
	match reply.status {
		200..=299 => reply
			.body
			.map_err(|_| Error::External("invalid remote authority response".into())),
		401 | 403 => Err(Error::Forbidden),
		// The receiver did not find it. That is no denial: the model hears it as a Rejected
		// Call, and an authority refresh maps it back to `Forbidden` itself.
		404 => Err(Error::NotFound("the target Node did not find it".into())),
		409 => Err(Error::Conflict("remote execution authority changed".into())),
		503 if reply.transaction_pending => Err(Error::TransactionPending),
		503 if reply.policy_busy.is_some() => Err(Error::PolicyBusy {
			retry_after: reply.policy_busy.unwrap_or(1),
		}),
		_ => Err(Error::External(
			"remote execution authority unavailable".into(),
		)),
	}
}
#[cfg(test)]
mod tests;
