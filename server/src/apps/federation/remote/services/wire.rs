//! How a request leaves this Node: in process to another Node of this
//! Deployment, or over HTTP with a Federation token to another Deployment
//! (ADR 0064). Every transport port is served by the same two paths.
use super::local;
use crate::database::native::distribution::{self, Hold};
use crate::federation::Federation;
use aidash_application::{
	Result,
	ports::federation::{
		PeerReply, PeerTransport,
		authority::{Reply, Transport},
		dependencies::{Check, DependencyTransport},
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
	/// The runtime whose handlers answer a Node of this Deployment. `None`
	/// reaches no other Node: such an exchange is not asked (#751).
	pub runtime: Option<Federation>,
	/// Nodes that did not answer a list read within the last moments, shared by
	/// every clone of the Store (#770).
	pub silent: std::sync::Arc<SilentNodes>,
}

impl Wire {
	pub(crate) fn of(runtime: &Federation) -> Self {
		Self {
			runtime: Some(runtime.clone()),
			..Self::of_store(&runtime.store)
		}
	}

	/// The transport of a caller that holds a Store. A server binds its
	/// in-process handlers to the Store (`Federation::bind_in_process`), so the
	/// caller reaches the other Nodes of this Deployment by default. In a
	/// process that never built a `Federation` it reaches none, and such an
	/// exchange is reported as not asked.
	pub(crate) fn of_store(store: &crate::store::Store) -> Self {
		Self {
			runtime: store
				.in_process
				.get()
				.map(|bound| bound.federation(store.clone())),
			..Self::without_peers(store)
		}
	}

	/// The transport of a caller that must not ask another Node, such as one
	/// holding an exclusive lock the peer's own read would wait on.
	pub(crate) fn without_peers(store: &crate::store::Store) -> Self {
		Self {
			http: HttpTransport {
				endpoints: Endpoints::new(store.keyless.clone())
					.expect("the federation endpoint clients build"),
				deployment: store.deployment.clone(),
				egress: store.egress.clone(),
			},
			runtime: None,
			silent: store.silent_nodes.clone(),
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
			unreachable: (outcome.status == 503)
				.then(|| {
					aidash_application::ports::federation::authority::Unreachable::of(&outcome.body)
				})
				.flatten(),
			policy_busy: outcome.policy_busy,
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
		if self.runtime.is_none() {
			tracing::debug!(node = %route.target, "a Node of this Deployment was not asked");
			return Verification::NotAsked;
		}
		let body = json!({"references":references});
		let answered = tokio::time::timeout(
			Duration::from_secs(10),
			self.local(route, "POST", "/scoped/registry/verify", Some(&body)),
		)
		.await;
		match answered {
			Ok(Ok(outcome)) if outcome.status == 200 => match outcome.body {
				Value::Bool(true) => Verification::Verified,
				Value::Bool(false) => Verification::Rejected,
				_ => unavailable(route, &"an unreadable answer"),
			},
			Ok(Ok(outcome)) if aidash_application::ports::federation::refuses(outcome.status) => {
				Verification::Rejected
			}
			Ok(Ok(outcome)) => unavailable(route, &format!("HTTP {}", outcome.status)),
			Ok(Err(error)) => unavailable(route, &error),
			Err(_) => unavailable(route, &"the request timed out"),
		}
	}
}

/// A peer of this Deployment that did not answer, with the Node and cause logged.
fn unavailable(route: &Route, cause: &dyn std::fmt::Display) -> Verification {
	tracing::warn!(node = %route.target, %cause, "a Node did not answer a check");
	Verification::Unavailable
}

#[async_trait]
impl DependencyTransport for Wire {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Check {
		if !route.local() {
			return self.http.check(route, input, remaining).await;
		}
		if self.runtime.is_none() {
			tracing::debug!(node = %route.target, "a Node of this Deployment was not asked");
			return Check::NotAsked;
		}
		let body = json!({"reference":input.reference});
		let answered = tokio::time::timeout(
			remaining.min(Duration::from_secs(10)),
			self.local(route, "POST", "/scoped/dependencies/verify", Some(&body)),
		)
		.await;
		let cause: &dyn std::fmt::Display = match &answered {
			Ok(Ok(outcome)) if outcome.status == 200 => {
				match serde_json::from_value::<Checked>(outcome.body.clone()) {
					Ok(checked) => return Check::Confirmed(checked),
					Err(_) => &"an unreadable answer",
				}
			}
			Ok(Ok(outcome)) => {
				let check = Check::of_status(outcome.status);
				if matches!(check, Check::Refused) {
					return check;
				}
				tracing::warn!(
					node = %route.target,
					cause = %format_args!("HTTP {}", outcome.status),
					"a Node did not answer a check"
				);
				return check;
			}
			Ok(Err(error)) => error,
			Err(_) => &"the request timed out",
		};
		tracing::warn!(node = %route.target, %cause, "a Node did not answer a check");
		Check::Unavailable
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
/// disclosure check reaches no other Node, so it asks nobody and reports that
/// it did not ask. Agents found on a Node of the reader's own Tenant need no
/// proof: policy decides them (#769). The second field is the distribution lock
/// the authority's transaction holds: a Node of this Deployment it asks in
/// process runs inside that transaction's lock and requests none of its own (#751).
pub(crate) struct Reach(pub Option<Wire>, pub Hold);

#[async_trait]
impl RegistryTransport for Reach {
	async fn verify(&self, route: &Route, references: &[Reference]) -> Verification {
		match &self.0 {
			Some(wire) => distribution::serving(&self.1, wire.verify(route, references)).await,
			None => {
				tracing::debug!(node = %route.target, "no transport; the Node was not asked");
				Verification::NotAsked
			}
		}
	}
}

#[async_trait]
impl DependencyTransport for Reach {
	async fn check(&self, route: &Route, input: &Input, remaining: Duration) -> Check {
		match &self.0 {
			Some(wire) => distribution::serving(&self.1, wire.check(route, input, remaining)).await,
			None => {
				tracing::debug!(node = %route.target, "no transport; the Node was not asked");
				Check::NotAsked
			}
		}
	}
}

mod silence;
pub(crate) use silence::SilentNodes;

#[cfg(test)]
mod tests;
