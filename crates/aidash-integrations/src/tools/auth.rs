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
	let value = match challenge {
		Challenge::Unauthorized { .. } => {
			json!({"error": CONNECTION_REQUIRED, "resource": resource, "_token_refused":true})
		}
		Challenge::InsufficientScope { scope } => {
			let mut value = json!({"error": CONNECTION_SCOPE, "resource": resource});
			if let Some(scope) = scope {
				value["scope"] = json!(scope);
			}
			value
		}
		Challenge::None => return None,
	};
	Some(Error::Invalid(value.to_string()))
}

pub(crate) fn error_value(error: &Error) -> Option<Value> {
	let Error::Invalid(message) = error else {
		return None;
	};
	let mut value: Value = serde_json::from_str(message).ok()?;
	value.as_object_mut()?.remove("_token_refused");
	(matches!(
		value["error"].as_str(),
		Some(CONNECTION_REQUIRED | CONNECTION_SCOPE | CONNECTION_UNAVAILABLE)
	) && value["resource"].is_string())
	.then_some(value)
}

/// Only a refused bearer permits replay: invalidate once and reopen the whole session.
pub(crate) async fn retry(error: &Error, auth: Option<&CallSecret>) -> bool {
	let Some(value) = error_value(error) else {
		return false;
	};
	let refused = match error {
		Error::Invalid(message) => serde_json::from_str::<Value>(message)
			.ok()
			.is_some_and(|value| value["_token_refused"] == true),
		_ => false,
	};
	if value["error"] != CONNECTION_REQUIRED || !refused {
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
			Error::Invalid(json!({"error":code,"resource":resource}).to_string())
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
