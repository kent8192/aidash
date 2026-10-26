//! Broker-owned Tenant Secret failures and where a brokered call goes
//! (ADR 0095). A response with `x-aidash-broker-error` is the broker speaking;
//! every other status is the tool endpoint's, passed through unchanged.
use aidash_application::{Error, Result};
use reqwest::header::HeaderMap;

/// The Tenant Secret Capability Token of one forwarded call.
pub(crate) const SECRET_CALL: &str = "x-aidash-secret-call";
const BROKER_ERROR: &str = "x-aidash-broker-error";
const CORRELATION: &str = "x-aidash-correlation";

const BROKER_REJECTED: &str = "broker_rejected";
const DESTINATION_REFUSED: &str = "secret_destination_refused";
const SECRET_UNAVAILABLE: &str = "secret_unavailable";
const RATE_LIMITED: &str = "rate_limited";
const BROKER_UNAVAILABLE: &str = "broker_unavailable";
const UPSTREAM_UNREACHABLE: &str = "upstream_unreachable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerCode {
	Rejected,
	Unavailable,
	SecretUnavailable,
	DestinationRefused,
	UpstreamUnreachable,
	RateLimited,
}
impl BrokerCode {
	pub(crate) fn as_str(self) -> &'static str {
		match self {
			Self::Rejected => BROKER_REJECTED,
			Self::Unavailable => BROKER_UNAVAILABLE,
			Self::SecretUnavailable => SECRET_UNAVAILABLE,
			Self::DestinationRefused => DESTINATION_REFUSED,
			Self::UpstreamUnreachable => UPSTREAM_UNREACHABLE,
			Self::RateLimited => RATE_LIMITED,
		}
	}
}

/// A broker-owned failure and the correlation id its operator can search for.
#[derive(Debug, Clone)]
pub(crate) struct BrokerFailure {
	pub(crate) code: BrokerCode,
	pub(crate) correlation: Option<String>,
}

impl BrokerFailure {
	/// Read a broker-owned failure. The reason code is logged with its
	/// correlation id for Deployment operators; it is never put in the error a
	/// Tenant sees.
	pub(crate) fn from_headers(headers: &HeaderMap) -> Option<Self> {
		let code = headers.get(BROKER_ERROR)?.to_str().ok()?;
		let code = match code {
			BROKER_REJECTED => BrokerCode::Rejected,
			BROKER_UNAVAILABLE => BrokerCode::Unavailable,
			SECRET_UNAVAILABLE => BrokerCode::SecretUnavailable,
			DESTINATION_REFUSED => BrokerCode::DestinationRefused,
			UPSTREAM_UNREACHABLE => BrokerCode::UpstreamUnreachable,
			RATE_LIMITED => BrokerCode::RateLimited,
			other => {
				tracing::warn!(
					code = other,
					"unknown Tenant Secret broker error code; treating it as an operator fault"
				);
				BrokerCode::Unavailable
			}
		};
		let correlation = headers
			.get(CORRELATION)
			.and_then(|value| value.to_str().ok())
			.map(str::to_owned);
		tracing::warn!(
			code = code.as_str(),
			correlation = correlation.as_deref().unwrap_or(""),
			"Tenant Secret broker refused a tool call"
		);
		Some(Self { code, correlation })
	}

	/// An unreachable broker is an operator fault too, with no correlation.
	pub(crate) fn connect_failure() -> Self {
		tracing::warn!(
			code = BROKER_UNAVAILABLE,
			"Tenant Secret broker could not be reached"
		);
		Self {
			code: BrokerCode::Unavailable,
			correlation: None,
		}
	}

	pub(crate) fn is_upstream_or_rate_limited(&self) -> bool {
		matches!(
			self.code,
			BrokerCode::UpstreamUnreachable | BrokerCode::RateLimited
		)
	}

	pub(crate) fn is_repairable(&self) -> bool {
		matches!(
			self.code,
			BrokerCode::SecretUnavailable | BrokerCode::DestinationRefused
		)
	}

	pub(crate) fn code_str(&self) -> &'static str {
		self.code.as_str()
	}

	/// The application error a caller sees. A repairable refusal keeps its code;
	/// an operator fault names only the correlation. The broker's reason code
	/// stays in the log above.
	pub(crate) fn into_error(self) -> Error {
		match self.code {
			BrokerCode::SecretUnavailable => Error::Invalid(SECRET_UNAVAILABLE.into()),
			BrokerCode::DestinationRefused => Error::Invalid(DESTINATION_REFUSED.into()),
			BrokerCode::Rejected => Error::BrokerRejected {
				correlation: self.correlation,
			},
			BrokerCode::Unavailable => Error::BrokerUnavailable {
				correlation: self.correlation,
			},
			// A broker that could not reach the destination is the tool's
			// upstream outage, as a direct call's connect failure is.
			BrokerCode::UpstreamUnreachable => {
				Error::External("Tenant Secret broker could not reach the tool endpoint".into())
			}
			// No tool-level rate-limit retry policy exists; keep it an outage.
			BrokerCode::RateLimited => {
				Error::External("Tenant Secret broker rate limited the call".into())
			}
		}
	}
}

/// `{broker}/v1/forward` followed by the endpoint path outside its signed
/// prefix. The broker joins that remainder under the signed prefix, so the
/// encoded path is passed through byte for byte rather than re-parsed.
pub(crate) fn forward_url(base: &reqwest::Url, rest: &str) -> Result<reqwest::Url> {
	if rest.is_empty() {
		return Ok(base.clone());
	}
	let mut raw = base.as_str().trim_end_matches('/').to_owned();
	raw.push('/');
	raw.push_str(rest);
	reqwest::Url::parse(&raw)
		.map_err(|_| Error::Invalid("invalid Tenant Secret broker forward URL".into()))
}
