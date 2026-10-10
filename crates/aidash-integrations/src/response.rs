//! Bounded decoding at external HTTP trust boundaries.
use crate::{Error, Result};
use serde::de::DeserializeOwned;

/// Only broker-owned failure codes on a BYOK call become non-retryable mint errors.
pub(crate) fn capability_failure(status: u16, body: Option<&serde_json::Value>) -> Option<Error> {
	if !matches!(status, 401 | 403) {
		return None;
	}
	let code = body?.pointer("/error/code")?.as_str()?;
	if !matches!(
		code,
		"capability_invalid_signature"
			| "capability_unknown_kid"
			| "capability_expired"
			| "capability_audience"
			| "capability_tenant"
			| "capability_credential"
			| "capability_operation"
			| "capability_model"
			| "capability_claim_violation"
	) {
		return None;
	}
	Some(Error::Invalid(format!(
		"Credential Broker rejected Capability Token ({code})"
	)))
}
pub(crate) async fn provider_rejection(response: reqwest::Response, byok: bool) -> Error {
	let status = response.status().as_u16();
	let body = json::<serde_json::Value>(response, 16_384).await.ok();
	rejection(status, body.as_ref(), byok)
}
/// Classifies a decoded non-2xx reply without exposing the upstream body.
pub(crate) fn rejection(status: u16, body: Option<&serde_json::Value>, byok: bool) -> Error {
	if byok && let Some(error) = capability_failure(status, body) {
		return error;
	}
	let detail = body
		.and_then(|body| body.pointer("/error/message"))
		.and_then(serde_json::Value::as_str)
		.unwrap_or_default();
	Error::ProviderRejected {
		status,
		reason: crate::inference::safe_upstream_reason(detail),
	}
}

pub(crate) async fn json<T: DeserializeOwned>(
	response: reqwest::Response,
	limit: usize,
) -> Result<T> {
	serde_json::from_slice(&bytes(response, limit).await?)
		.map_err(|error| Error::External(format!("invalid response JSON: {error}")))
}

pub(crate) async fn bytes(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
	if response
		.content_length()
		.is_some_and(|length| length > limit as u64)
	{
		return Err(Error::External(format!("response exceeds {limit} bytes")));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(crate::http_error)? {
		if chunk.len() > limit.saturating_sub(bytes.len()) {
			return Err(Error::External(format!("response exceeds {limit} bytes")));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	#[test]
	fn only_fixed_broker_codes_and_statuses_are_non_retryable_configuration_errors() {
		for reason in [
			"invalid_signature",
			"unknown_kid",
			"expired",
			"audience",
			"tenant",
			"credential",
			"operation",
			"model",
			"claim_violation",
		] {
			let body = serde_json::json!({"error":{"code":format!("capability_{reason}"),"message":"canary-key"}});
			for status in [401, 403] {
				let error = super::capability_failure(status, Some(&body)).unwrap();
				assert!(matches!(error, aidash_application::Error::Invalid(_)));
				assert!(!error.to_string().contains("canary-key"));
			}
			assert!(super::capability_failure(500, Some(&body)).is_none());
		}
		assert!(
			super::capability_failure(
				401,
				Some(&serde_json::json!({"error":{"code":"capability_canary-key"}}))
			)
			.is_none()
		);
		assert!(
			super::capability_failure(
				401,
				Some(&serde_json::json!({"error":{"message":"upstream unauthorized"}}))
			)
			.is_none()
		);
	}
}
