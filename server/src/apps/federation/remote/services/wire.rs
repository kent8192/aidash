//! How a request leaves this Node: in process to another Node of this
//! Deployment, or over HTTP with a Federation token to another Deployment
//! (ADR 0064). Every transport port is served by the same two paths.
use super::local;
use crate::federation::Federation;
use aidash_application::{
	Result,
	ports::federation::{
		PeerReply, PeerTransport,
		authority::{Reply, Transport},
		dependencies::DependencyTransport,
		registry_reads::{RegistryTransport, Verification},
	},
};
use aidash_domain::federation::{
	dependencies::{Checked, Input},
	registry_reads::Reference,
	route::Route,
};
use aidash_integrations::federation::{HttpTransport, endpoint::Endpoints};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct Wire {
	pub http: HttpTransport,
	/// Present when the caller holds the runtime; a caller holding only a
	/// Store has no way into this process's handlers.
	pub runtime: Option<Federation>,
}

impl Wire {
	pub(crate) fn of(runtime: &Federation) -> Self {
		Self {
			runtime: Some(runtime.clone()),
			..Self::of_store(&runtime.store)
		}
	}

	/// The transport of a caller that holds a Store: exchange inside this
	/// Deployment is refused as unavailable, because only the runtime reaches
	/// the receiving handlers.
	pub(crate) fn of_store(store: &crate::store::Store) -> Self {
		Self {
			http: HttpTransport {
				endpoints: Endpoints::new(store.keyless.clone())
					.expect("the federation endpoint clients build"),
				deployment: store.deployment.clone(),
				egress: store.egress.clone(),
			},
			runtime: None,
		}
	}

	async fn local(
		&self,
		route: &Route,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<local::Outcome> {
		let Some(runtime) = &self.runtime else {
			return Err(aidash_application::Error::External(
				"in-process Federation needs the runtime".into(),
			));
		};
		local::call(runtime, route, method, path, body)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl PeerTransport for Wire {
	async fn request(
		&self,
		route: &Route,
		method: &str,
		path: &str,
		body: Option<&Value>,
	) -> Result<PeerReply> {
		if !route.local() {
			return PeerTransport::request(&self.http, route, method, path, body).await;
		}
		let outcome = self.local(route, method, path, body).await?;
		Ok(PeerReply {
			status: outcome.status,
			status_label: outcome.status.to_string(),
			transaction_pending: outcome.transaction_pending,
			run_message_pending: outcome.run_message_pending,
			body: outcome.body,
		})
	}
}

#[async_trait]
impl Transport for Wire {
	async fn request(&self, route: &Route, path: &str, body: &Value) -> Result<Reply> {
		if !route.local() {
			return Transport::request(&self.http, route, path, body).await;
		}
		let outcome = self.local(route, "POST", path, Some(body)).await?;
		Ok(Reply {
			status: outcome.status,
			semantic_reason: outcome.semantic_reason,
			execution_refusal: outcome.execution_refusal,
			transaction_pending: outcome.transaction_pending,
			body: if (200..300).contains(&outcome.status) {
				serde_json::to_vec(&outcome.body).map_err(Into::into)
			} else {
				Ok(Vec::new())
			},
		})
	}
}

#[async_trait]
impl RegistryTransport for Wire {
	async fn verify(&self, route: &Route, references: &[Reference]) -> Verification {
		if !route.local() {
			return self.http.verify(route, references).await;
		}
		let body = json!({"references":references});
		match self
			.local(route, "POST", "/scoped/registry/verify", Some(&body))
			.await
		{
			Ok(outcome) if outcome.status == 200 && outcome.body == json!(true) => {
				Verification::Verified
			}
			Ok(outcome) if outcome.status == 200 => Verification::Rejected,
			_ => Verification::Unavailable,
		}
	}
}

#[async_trait]
impl DependencyTransport for Wire {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Option<Checked> {
		if !route.local() {
			return self.http.check(route, input, remaining).await;
		}
		let body = json!({"reference":input.reference});
		let outcome = tokio::time::timeout(
			remaining.min(Duration::from_secs(10)),
			self.local(route, "POST", "/scoped/dependencies/verify", Some(&body)),
		)
		.await
		.ok()?
		.ok()?;
		if outcome.status != 200 {
			return None;
		}
		serde_json::from_value(outcome.body).ok()
	}
}

impl Wire {
	/// An atomic transaction RPC, with the same ten-second deadline over either path.
	pub(crate) async fn transaction<T: serde::de::DeserializeOwned>(
		&self,
		route: impl std::future::Future<Output = Result<Route>>,
		method: &str,
		path: &str,
		body: Option<&impl serde::Serialize>,
	) -> Result<T> {
		let value = body.map(serde_json::to_value).transpose()?;
		let route = route.await?;
		if !route.local() {
			return self
				.http
				.transaction(async { Ok(route) }, method, path, body)
				.await;
		}
		let outcome = tokio::time::timeout(
			Duration::from_secs(10),
			self.local(&route, method, path, value.as_ref()),
		)
		.await
		.map_err(|_| {
			aidash_application::Error::External(
				"transaction participant response timed out; outcome retained for recovery".into(),
			)
		})??;
		local::classify_transaction(outcome)
	}
}

/// The transport of an authority that may hold none: a transaction lent to a
/// disclosure check reaches no other Node, so its remote proofs are unavailable.
pub(crate) struct Reach(pub Option<Wire>);

#[async_trait]
impl RegistryTransport for Reach {
	async fn verify(&self, route: &Route, references: &[Reference]) -> Verification {
		match &self.0 {
			Some(wire) => wire.verify(route, references).await,
			None => Verification::Unavailable,
		}
	}
}

#[async_trait]
impl DependencyTransport for Reach {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Option<Checked> {
		self.0.as_ref()?.check(route, input, remaining).await
	}
}
