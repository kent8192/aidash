//! Bounded HTTP transport for Federation v0.1.
use aidash_application::{
	Error, Result,
	ports::{
		Credentials,
		federation::{PeerReply, PeerTransport},
	},
};
use aidash_domain::federation::Peer;
use async_trait::async_trait;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

pub struct PeerHttp {
	pub client: reqwest::Client,
	pub node_id: String,
	pub protocol_version: String,
	pub credentials: Arc<dyn Credentials>,
}

#[async_trait]
impl aidash_application::ports::federation::peers::PeerIdentity for PeerHttp {
	async fn identity(&self, peer: &Peer) -> Result<Value> {
		PeerHttp::identity(self, peer).await
	}
}
impl PeerHttp {
	/// Outer HTTP adapters can inspect response headers before decoding their
	/// own protocol. Credentials are resolved at every send, including rotation.
	pub async fn send(
		&self,
		peer: &Peer,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<reqwest::Response> {
		// Composite authority operations make further bounded peer calls. Native
		// retrieval also executes separately bounded provider attempts before its
		// receipt is finalized; the outer request must allow those calls to finish.
		let timeout = if path == "/scoped/semantic/query" {
			Duration::from_secs(120)
		} else if path == "/scoped/execution/commands"
			|| path.starts_with("/scoped/execution/grants/")
			|| path.starts_with("/scoped/execution/admissions")
			|| path.starts_with("/scoped/usage/")
		{
			Duration::from_secs(60)
		} else {
			Duration::from_secs(10)
		};
		let method = reqwest::Method::from_bytes(method.as_bytes())
			.map_err(|_| Error::Invalid("invalid peer method".into()))?;
		let mut request = self
			.client
			.request(
				method,
				format!(
					"{}/federation/v0.1{path}",
					peer.endpoint.trim_end_matches('/')
				),
			)
			.timeout(timeout)
			.bearer_auth(self.credentials.resolve(&peer.credential_env)?)
			.header("x-aidash-node", &self.node_id)
			.header("x-aidash-protocol", &self.protocol_version);
		if let Some(body) = body {
			request = request.json(body);
		}
		request.send().await.map_err(crate::http_error)
	}
	pub async fn identity(&self, peer: &Peer) -> Result<Value> {
		let response = self
			.client
			.get(format!(
				"{}/.well-known/aidash",
				peer.endpoint.trim_end_matches('/')
			))
			.timeout(Duration::from_secs(5))
			.send()
			.await
			.map_err(crate::http_error)?
			.error_for_status()
			.map_err(crate::http_error)?;
		crate::response::json(response, 1_048_576).await
	}
}
#[async_trait]
impl PeerTransport for PeerHttp {
	async fn request(
		&self,
		peer: &Peer,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply> {
		let response = self.send(peer, method, path, body).await?;
		let status = response.status();
		let transaction_pending = response
			.headers()
			.get("x-aidash-transaction-pending")
			.is_some_and(|value| value == "1");
		let run_message_pending = response
			.headers()
			.contains_key("x-aidash-run-message-pending");
		let body = if status.is_success() {
			crate::response::json(response, 4_194_304).await?
		} else if status == reqwest::StatusCode::BAD_REQUEST {
			crate::response::json(response, 4_194_304)
				.await
				.unwrap_or(Value::Null)
		} else {
			Value::Null
		};
		Ok(PeerReply {
			status: status.as_u16(),
			status_label: status.to_string(),
			transaction_pending,
			run_message_pending,
			body,
		})
	}
}

#[cfg(test)]
mod tests;

mod dependencies;

mod registry_reads;

mod authority;

mod transactions;
