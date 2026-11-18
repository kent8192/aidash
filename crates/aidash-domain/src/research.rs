//! Page reading: one public Web page, read again on every call and returned
//! in bounded parts. Nothing is kept beyond the Run's Context Journal (#987).
//! The module keeps the `research` name only because the built-in it serves,
//! `research.page@1`, keeps its stored identifier.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod address;
pub mod document;
pub mod intent;

/// Upper bound of one page reading result as recorded in a Run's history.
pub const MAX_RESULT_BYTES: usize = 32 * 1024;

/// The one operation of page reading. The enum keeps the stored `fetch` name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
	/// Page reading: reads one page and returns one part of its text.
	Fetch,
}

impl Operation {
	pub fn parse(operation: &str) -> Option<Self> {
		(operation == "fetch").then_some(Self::Fetch)
	}
	pub fn as_str(self) -> &'static str {
		"fetch"
	}
}

/// Stable failure codes of page reading. Explanations never contain response
/// bodies or credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
	InvalidInput,
	/// The page has fewer parts than the one asked for.
	NoSuchPart,
	NonPublicDestination,
	ProviderAuth,
	RateLimited,
	ProviderUnavailable,
	ProviderRequestRejected,
	ProviderResponseInvalid,
	ResponseTooLarge,
	UnsupportedMedia,
	ExtractionFailed,
	/// The document opens only with a password.
	EncryptedDocument,
	/// The document's permissions forbid extracting its text.
	ExtractionNotPermitted,
	/// The page redirected to another host.
	CrossHostRedirect,
	/// The page redirected from https to http.
	InsecureRedirect,
	Timeout,
	TransportError,
	ResponseStreamError,
	/// The call was not carried out and nothing was sent.
	Refused,
}

impl Failure {
	pub fn code(self) -> &'static str {
		match self {
			Self::InvalidInput => "invalid_input",
			Self::NoSuchPart => "no_such_part",
			Self::NonPublicDestination => "non_public_destination",
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
			Self::Refused => "refused",
		}
	}
	fn explanation(self) -> &'static str {
		match self {
			Self::InvalidInput => {
				"The request needs an http(s) url and, optionally, a part of 1 or more."
			}
			Self::NoSuchPart => "The page has fewer parts than the one asked for.",
			Self::NonPublicDestination => "The destination is not a public Web address.",
			Self::ProviderAuth => "The page refused the request.",
			Self::RateLimited => "The site's rate limit was reached.",
			Self::ProviderUnavailable => "The site is temporarily unavailable.",
			Self::ProviderRequestRejected => "The site rejected the request.",
			Self::ProviderResponseInvalid => "The site returned an invalid response.",
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
			Self::Timeout => "The deadline expired before the page answered.",
			Self::TransportError => "The connection to the site failed.",
			Self::ResponseStreamError => "The response was interrupted.",
			Self::Refused => "The call was not carried out and nothing was sent.",
		}
	}
	pub fn retryable(self) -> bool {
		matches!(
			self,
			Self::RateLimited | Self::ProviderUnavailable | Self::Timeout | Self::TransportError
		)
	}
}

/// Names the page reading result contract in every result.
pub const RESULT_CONTRACT: &str = "aidash.page/1";

/// The result envelope of page reading.
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

/// A recoverable page reading error returned to the model as an ordinary result.
pub fn failure(operation: Operation, failure: Failure) -> Value {
	json!({
		"contract": RESULT_CONTRACT,
		"version": 1,
		"operation": operation.as_str(),
		"status": "error",
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

/// Bytes `c` takes inside a JSON string: quotes, backslashes and the control
/// characters with a short escape take two, other control characters six.
fn escaped_char_len(c: char) -> usize {
	match c {
		'"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
		c if u32::from(c) < 0x20 => 6,
		c => c.len_utf8(),
	}
}

/// Bytes `text` takes inside a JSON string, as a recorded result encodes it.
pub fn escaped_len(text: &str) -> usize {
	text.chars().map(escaped_char_len).sum()
}

/// The longest prefix of `text` whose JSON encoding fits in `max` bytes, and
/// at least one character, so that splitting always advances.
fn escaped_prefix(text: &str, max: usize) -> &str {
	let mut used = 0;
	for (index, c) in text.char_indices() {
		used += escaped_char_len(c);
		if used > max {
			let end = if index == 0 { c.len_utf8() } else { index };
			return &text[..end];
		}
	}
	text
}

/// The text of a page split into parts whose JSON encoding takes at most `max`
/// bytes, at line breaks where a line fits and at character boundaries where
/// it does not. Sizing by the encoded length keeps a part of quotes,
/// backslashes or short lines within the result bound it is returned in.
pub fn parts(text: &str, max: usize) -> Vec<String> {
	let mut parts = Vec::new();
	let mut current = String::new();
	let mut current_len = 0;
	for line in text.split_inclusive('\n') {
		let mut line = line;
		let mut line_len = escaped_len(line);
		while current_len + line_len > max {
			if !current.is_empty() {
				parts.push(std::mem::take(&mut current));
				current_len = 0;
				continue;
			}
			let head = escaped_prefix(line, max);
			parts.push(head.to_owned());
			line = &line[head.len()..];
			line_len -= escaped_len(head);
		}
		current.push_str(line);
		current_len += line_len;
	}
	if !current.is_empty() || parts.is_empty() {
		parts.push(current);
	}
	parts
}

#[cfg(test)]
mod tests;
