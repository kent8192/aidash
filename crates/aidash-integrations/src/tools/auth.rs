//! MCP authentication challenges retain structured, model-visible context across SDK errors.
use aidash_application::ports::oauth::{Challenge, parse_challenge};
use aidash_application::{Error, tenant_secrets::CallSecret};
use aidash_domain::oauth_connections::{
	CONNECTION_REQUIRED, CONNECTION_SCOPE, CONNECTION_UNAVAILABLE,
};
use serde_json::{Value, json};

pub(crate) fn challenge(response: &reqwest::Response, endpoint: &str) -> Option<Error> {
	let challenge = parse_challenge(
		response.status().as_u16(),
		response
			.headers()
			.get(reqwest::header::WWW_AUTHENTICATE)
			.and_then(|v| v.to_str().ok()),
	);
	let resource = aidash_domain::oauth_connections::canonical_resource(endpoint).ok()?;
	let cause = match challenge {
		Challenge::Unauthorized { .. } => {
			aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired { resource }
		}
		Challenge::InsufficientScope { scope } => {
			aidash_domain::agent_plugin::ConnectionCause::ConnectionScopeInsufficient {
				resource,
				scope,
			}
		}
		Challenge::None => return None,
	};
	Some(
		if matches!(
			cause,
			aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired { .. }
		) {
			Error::Port(Box::new(RefusedBearer(cause)))
		} else {
			Error::Connection(cause)
		},
	)
}

/// Store outages are ordinary failures, never a waitable authentication refusal.
pub(crate) fn error_value(error: &Error) -> Option<Value> {
	let Error::Invalid(message) = error else {
		return None;
	};
	let value: Value = serde_json::from_str(message).ok()?;
	(value["error"] == CONNECTION_UNAVAILABLE && value["resource"].is_string()).then_some(value)
}

/// Private transport provenance: an actual server refused a bearer. It is
/// normalized to Connection only after the single refresh attempt is decided.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", .0.code())]
struct RefusedBearer(aidash_domain::agent_plugin::ConnectionCause);
fn refused(error: &Error) -> Option<&RefusedBearer> {
	if let Error::Port(error) = error {
		error.downcast_ref::<RefusedBearer>()
	} else {
		None
	}
}
pub(crate) fn is_connection(error: &Error) -> bool {
	matches!(error, Error::Connection(_)) || refused(error).is_some()
}
pub(crate) fn repeat(error: &Error) -> Option<Error> {
	if let Some(refused) = refused(error) {
		return Some(Error::Port(Box::new(refused.clone())));
	}
	match error {
		Error::Connection(cause) => Some(Error::Connection(cause.clone())),
		Error::Invalid(message) => Some(Error::Invalid(message.clone())),
		_ => None,
	}
}
pub(crate) fn finish(error: Error) -> Error {
	if let Some(refused) = refused(&error) {
		Error::Connection(refused.0.clone())
	} else {
		error
	}
}
pub(crate) async fn retry(error: &Error, auth: Option<&CallSecret>) -> bool {
	if refused(error).is_none() {
		return false;
	}
	if let Some(CallSecret::Connection { source, .. }) = auth {
		source.invalidate().await;
		true
	} else if let Some(CallSecret::BrokeredConnection { source, .. }) = auth {
		source.invalidate().await;
		true
	} else {
		false
	}
}

/// A forced refresh can discover revoked consent before another HTTP request is sent.
pub(crate) fn source_error(error: Error, endpoint: &str) -> Error {
	match error {
		Error::Invalid(code) | Error::External(code)
			if matches!(
				code.as_str(),
				CONNECTION_REQUIRED | CONNECTION_SCOPE | CONNECTION_UNAVAILABLE
			) =>
		{
			let resource = aidash_domain::oauth_connections::canonical_resource(endpoint)
				.unwrap_or_else(|_| endpoint.to_owned());
			match code.as_str() {
				CONNECTION_REQUIRED => Error::Connection(
					aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired { resource },
				),
				CONNECTION_SCOPE => Error::Connection(
					aidash_domain::agent_plugin::ConnectionCause::ConnectionScopeInsufficient {
						resource,
						scope: None,
					},
				),
				_ => Error::Invalid(json!({"error":code,"resource":resource}).to_string()),
			}
		}
		other => other,
	}
}

/// A bearer source is confined to the canonical resource recorded when it was resolved.
pub(crate) fn validate(
	auth: Option<&CallSecret>,
	endpoint: &str,
) -> aidash_application::Result<()> {
	if let Some(
		CallSecret::Connection { record, .. } | CallSecret::BrokeredConnection { record, .. },
	) = auth
	{
		let resource = aidash_domain::oauth_connections::canonical_resource(endpoint)?;
		if resource != record.resource {
			return Err(Error::Forbidden);
		}
	}
	Ok(())
}

/// A connection code promises that the operation did not execute. Once a tools/call
/// request was accepted (including an input-required 2xx), later stream/continuation
/// auth failures are ordinary MCP failures: redispatch could repeat an effect.
#[derive(Default)]
pub(crate) struct CallProgress {
	in_flight: std::sync::atomic::AtomicBool,
	accepted: std::sync::atomic::AtomicBool,
}
impl CallProgress {
	pub(crate) fn begin(&self, previously_accepted: bool) {
		use std::sync::atomic::Ordering::SeqCst;
		self.in_flight.store(previously_accepted, SeqCst);
		self.accepted.store(previously_accepted, SeqCst);
	}
	pub(crate) fn sent(&self) {
		self.in_flight
			.store(true, std::sync::atomic::Ordering::SeqCst);
	}
	pub(crate) fn challenge(
		&self,
		response: &reqwest::Response,
		endpoint: &str,
		tool_response: bool,
	) -> Option<Error> {
		use std::sync::atomic::Ordering::SeqCst;
		let accepted = self.accepted.load(SeqCst);
		// A definitive auth refusal of this tools/call is safe. A background
		// request's auth refusal while the tool is in flight has an uncertain outcome.
		let safe = !accepted && (tool_response || !self.in_flight.load(SeqCst));
		let error = if safe {
			challenge(response, endpoint)
		} else {
			None
		};
		if tool_response && response.status().is_success() {
			self.accepted.store(true, SeqCst);
		}
		error
	}
	pub(crate) fn source_error(&self, error: Error, endpoint: &str) -> Error {
		use std::sync::atomic::Ordering::SeqCst;
		if self.accepted.load(SeqCst) || self.in_flight.load(SeqCst) {
			Error::External("MCP authentication failed after the tool request was sent".into())
		} else {
			source_error(error, endpoint)
		}
	}
}
