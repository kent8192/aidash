//! Bounded HTTP transport for Federation v0.5 (ADR 0064). Every request carries
//! a Federation token minted for it, signed with the Deployment Key.
use aidash_application::{
	Error, Result,
	federation::key::{DeploymentKey, Signing},
	ports::federation::{PeerReply, PeerTransport},
};
use aidash_capability::{
	TOKEN_TTL_SECS,
	federation::{FederationClaims, body_digest},
};
use aidash_domain::{
	federation::{protocol::CURRENT, route::Route},
	nodes::{DeploymentId, registration::Counterpart},
};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// What a Deployment sends other Deployments as: its key and its switch.
#[derive(Clone, Default)]
pub struct Egress {
	/// `[deployment] federation`.
	pub enabled: bool,
	pub key: Option<Arc<dyn DeploymentKey>>,
}

#[derive(Clone)]
pub struct HttpTransport {
	pub endpoints: endpoint::Endpoints,
	/// This Deployment, the `iss` of every token.
	pub deployment: DeploymentId,
	pub egress: Egress,
}

impl HttpTransport {
	/// Outer HTTP adapters can inspect response headers before decoding their
	/// own protocol. The token is minted for this exact method, path and body
	/// right before it is sent. A route to a Node of this Deployment never comes
	/// here, and nothing is sent while Federation with other Deployments is off.
	pub async fn send(
		&self,
		route: &Route,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<reqwest::Response> {
		let Counterpart::OtherDeployment { endpoint, .. } = &route.counterpart else {
			return Err(Error::Invalid(
				"a Node of this Deployment is reached in process".into(),
			));
		};
		self.dispatch(
			endpoint,
			Party {
				sub: route.sender.to_string(),
				aud: route.target.to_string(),
				subject: route.subject.clone().unwrap_or_default(),
			},
			method,
			path,
			body,
		)
		.await
	}

	async fn dispatch(
		&self,
		endpoint: &str,
		party: Party,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<reqwest::Response> {
		let (true, Some(key)) = (self.egress.enabled, &self.egress.key) else {
			return Err(Error::FederationDisabled);
		};
		let client = self.endpoints.client(endpoint)?;
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
		let bytes = body
			.map(serde_json::to_vec)
			.transpose()?
			.unwrap_or_default();
		let now = chrono::Utc::now().timestamp().max(0) as u64;
		let claims = FederationClaims {
			iss: self.deployment.to_string(),
			sub: party.sub,
			aud: party.aud,
			subject: party.subject,
			iat: now,
			exp: now + TOKEN_TTL_SECS,
			jti: uuid::Uuid::new_v4(),
			kid: key.key_id().to_string(),
			protocol: CURRENT.into(),
			method: method.into(),
			path: path.into(),
			body: body_digest(&bytes),
		};
		let token = claims
			.mint(&Signing(key.as_ref()))
			.await
			.map_err(|_| Error::External("the Deployment Key could not sign".into()))?;
		let method = reqwest::Method::from_bytes(method.as_bytes())
			.map_err(|_| Error::Invalid("invalid peer method".into()))?;
		let mut request = client
			.request(
				method,
				format!("{}/federation/v0.1{path}", endpoint.trim_end_matches('/')),
			)
			.timeout(timeout)
			.bearer_auth(token.expose_secret())
			.header("x-aidash-protocol", CURRENT);
		if body.is_some() {
			request = request
				.header("content-type", "application/json")
				.body(bytes);
		}
		request.send().await.map_err(crate::keyless::send_error)
	}
}

/// The three claims that differ between a Node's token and a viewer's.
struct Party {
	sub: String,
	aud: String,
	subject: String,
}
#[async_trait]
impl PeerTransport for HttpTransport {
	async fn request(
		&self,
		route: &Route,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply> {
		let response = self.send(route, method, path, body).await?;
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
		} else if status == reqwest::StatusCode::BAD_REQUEST
			|| (status == reqwest::StatusCode::CONFLICT && path.starts_with("/scoped/previews/"))
		{
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
pub mod directory;
pub mod endpoint;
pub mod key;

mod registry_reads;

mod authority;

mod transactions;
