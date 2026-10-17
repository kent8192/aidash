//! Host-owned Web research contract. Provider packages perform Web search or
//! Page reading; the host validates their output, retains Research evidence and
//! checks Source citations. Nothing here names a provider or a Model alias.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod account;
pub mod citations;
pub mod document;
pub mod evidence;
pub mod expiry;
pub mod intent;
pub mod revocation;
pub mod search;

/// Upper bound of one research result as recorded in a Run's history.
pub const MAX_RESULT_BYTES: usize = 32 * 1024;

/// The host operations of the research contract. A provider package declares
/// which of them it implements; the operation, not the alias, selects the
/// host behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
	/// Web search: discovers Source records, never evidence.
	Search,
	/// Page reading: retrieves one Document snapshot and delivers fragments.
	Fetch,
	/// Locates passages in a retained Document snapshot. No disclosure.
	Find,
}

impl Operation {
	pub fn parse(operation: &str) -> Option<Self> {
		Some(match operation {
			"search" => Self::Search,
			"fetch" => Self::Fetch,
			"find" => Self::Find,
			_ => return None,
		})
	}
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Search => "search",
			Self::Fetch => "fetch",
			Self::Find => "find",
		}
	}
	/// Search and fetch send Run-derived content outside the Node.
	pub fn discloses(self) -> bool {
		!matches!(self, Self::Find)
	}
}

/// Stable failure codes of the research contract. Explanations never contain
/// provider response bodies or credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
	InvalidInput,
	UnknownSource,
	UnknownDocument,
	InvalidCursor,
	DisclosureDenied,
	NonPublicDestination,
	AttemptLimit,
	SourceLimit,
	SnapshotLimit,
	BudgetExhausted,
	AccountUnavailable,
	ProviderAuth,
	RateLimited,
	ProviderUnavailable,
	ProviderRequestRejected,
	ProviderResponseInvalid,
	ResponseTooLarge,
	UnsupportedMedia,
	ExtractionFailed,
	/// A PDF that opens only with a password.
	EncryptedDocument,
	/// A PDF whose permissions forbid extracting its text.
	ExtractionNotPermitted,
	/// A page redirected to another host than the approved destination.
	CrossHostRedirect,
	/// A page redirected from https to an unencrypted address.
	InsecureRedirect,
	Timeout,
	TransportError,
	ResponseStreamError,
	/// A dispatched attempt has no recorded outcome; it is never repeated.
	OutcomeUnknown,
	/// The Document snapshot's contents were purged; read its Source again.
	SnapshotPurged,
	/// The Run's Research evidence was revoked; the Run can do no more research.
	EvidenceRevoked,
	/// The Research account terms no longer keep this result (ADR 0058).
	ResultExpired,
}

impl Failure {
	pub fn code(self) -> &'static str {
		match self {
			Self::InvalidInput => "invalid_input",
			Self::UnknownSource => "unknown_source",
			Self::UnknownDocument => "unknown_document",
			Self::InvalidCursor => "invalid_cursor",
			Self::DisclosureDenied => "disclosure_denied",
			Self::NonPublicDestination => "non_public_destination",
			Self::AttemptLimit => "attempt_limit",
			Self::SourceLimit => "source_limit",
			Self::SnapshotLimit => "snapshot_limit",
			Self::BudgetExhausted => "budget_exhausted",
			Self::AccountUnavailable => "account_unavailable",
			Self::ProviderAuth => "provider_auth",
			Self::RateLimited => "rate_limited",
			Self::ProviderUnavailable => "provider_unavailable",
			Self::ProviderRequestRejected => "provider_request_rejected",
			Self::ProviderResponseInvalid => "provider_response_invalid",
			Self::ResponseTooLarge => "response_too_large",
			Self::UnsupportedMedia => "unsupported_media",
			Self::ExtractionFailed => "extraction_failed",
			Self::EncryptedDocument => "encrypted_document",
			Self::ExtractionNotPermitted => "extraction_not_permitted",
			Self::CrossHostRedirect => "cross_host_redirect",
			Self::InsecureRedirect => "insecure_redirect",
			Self::Timeout => "timeout",
			Self::TransportError => "transport_error",
			Self::ResponseStreamError => "response_stream_error",
			Self::OutcomeUnknown => "outcome_unknown",
			Self::SnapshotPurged => "snapshot_purged",
			Self::EvidenceRevoked => "evidence_revoked",
			Self::ResultExpired => "result_expired",
		}
	}
	fn explanation(self) -> &'static str {
		match self {
			Self::InvalidInput => "The request does not satisfy the research contract.",
			Self::UnknownSource => "The source is not a Source record of this Run.",
			Self::UnknownDocument => "The document is not a Document snapshot of this Run.",
			Self::InvalidCursor => "The continuation does not belong to this request.",
			Self::DisclosureDenied => "Web disclosure approval was denied or has expired.",
			Self::NonPublicDestination => "The destination is not a public Web address.",
			Self::AttemptLimit => "The Run reached its research attempt limit.",
			Self::SourceLimit => "The Run reached its Source record limit.",
			Self::SnapshotLimit => "The Run reached its Document snapshot limit.",
			Self::BudgetExhausted => "The search service account budget is exhausted.",
			Self::AccountUnavailable => "The search service account terms are unavailable.",
			Self::ProviderAuth => "The provider rejected the configured credential.",
			Self::RateLimited => "The provider rate limit was reached.",
			Self::ProviderUnavailable => "The provider is temporarily unavailable.",
			Self::ProviderRequestRejected => "The provider rejected the request.",
			Self::ProviderResponseInvalid => "The provider returned an invalid response.",
			Self::ResponseTooLarge => "The response exceeded the configured size limit.",
			Self::UnsupportedMedia => "The page has an unsupported media type.",
			Self::ExtractionFailed => "No readable text could be extracted from the page.",
			Self::EncryptedDocument => "The document opens only with a password.",
			Self::ExtractionNotPermitted => {
				"The document's permissions forbid extracting its text."
			}
			Self::CrossHostRedirect => {
				"The page redirected to another host; request that address in a new call."
			}
			Self::InsecureRedirect => "The page redirected from https to an unencrypted address.",
			Self::Timeout => "The deadline expired; the provider outcome is unknown.",
			Self::TransportError => "The transport failed; the provider outcome is unknown.",
			Self::ResponseStreamError => {
				"The response was interrupted; the provider outcome is unknown."
			}
			Self::OutcomeUnknown => {
				"An earlier attempt of this call has no recorded outcome and is not repeated."
			}
			Self::SnapshotPurged => {
				"The snapshot's contents are no longer retained; read its source again to continue."
			}
			Self::EvidenceRevoked => {
				"This Run's Research evidence was revoked; it cannot be read, cited or extended."
			}
			Self::ResultExpired => {
				"The research service's account terms no longer let this result be kept; search or read again to continue."
			}
		}
	}
	pub fn retryable(self) -> bool {
		matches!(self, Self::RateLimited | Self::ProviderUnavailable)
	}
	/// The provider may have performed the request; its cost is attributed.
	pub fn uncertain(self) -> bool {
		matches!(
			self,
			Self::Timeout | Self::TransportError | Self::ResponseStreamError | Self::OutcomeUnknown
		)
	}
}

/// Names the research result contract in every result, so a stored result is
/// recognized as research by this field alone, never by fields an unrelated
/// tool may also return.
pub const RESULT_CONTRACT: &str = "aidash.research/1";

/// The result envelope shared by every research operation.
pub fn envelope(operation: Operation, status: &str, data: Value, truncated: bool) -> Value {
	json!({
		"contract": RESULT_CONTRACT,
		"version": 1,
		"operation": operation.as_str(),
		"status": status,
		"limits": {"max_bytes": MAX_RESULT_BYTES, "truncated": truncated},
		"data": data,
	})
}

/// A recoverable research error returned to the model as an ordinary result.
pub fn failure(operation: Operation, failure: Failure) -> Value {
	json!({
		"contract": RESULT_CONTRACT,
		"version": 1,
		"operation": operation.as_str(),
		"status": if failure.uncertain() { "uncertain" } else { "error" },
		"error": {
			"code": failure.code(),
			"explanation": failure.explanation(),
			"retryable": failure.retryable(),
		},
		"limits": {"max_bytes": MAX_RESULT_BYTES, "truncated": false},
	})
}

/// Truncate at a character boundary; reports whether anything was cut.
pub fn bounded(value: &str, max: usize) -> (&str, bool) {
	if value.len() <= max {
		return (value, false);
	}
	let mut end = max;
	while !value.is_char_boundary(end) {
		end -= 1;
	}
	(&value[..end], true)
}

#[cfg(test)]
mod tests;
