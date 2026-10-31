//! Per-attempt inference usage. Reported counts, provider cost and Aidash's
//! request estimate stay separate; an absent value is unknown, never zero.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Usage exactly as the provider reported it. Each field is `None` when the
/// provider omitted it or sent a value that is not a valid count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportedUsage {
	pub input_tokens: Option<u64>,
	pub output_tokens: Option<u64>,
	/// Breakdown of `input_tokens` served from the provider's prompt cache.
	pub cache_read_tokens: Option<u64>,
	/// Breakdown of `input_tokens` written to the provider's prompt cache.
	pub cache_write_tokens: Option<u64>,
	/// Breakdown of `output_tokens` spent on reasoning.
	pub reasoning_tokens: Option<u64>,
	pub cost: Option<ProviderCost>,
	/// Only identities explicitly reported upstream, never requested values.
	pub provider_identity: Option<ProviderIdentity>,
}

/// Allowlisted response metadata. Missing or invalid fields remain unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderIdentity {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reported_model: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reported_provider: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reported_route: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider_request_id: Option<String>,
}

/// Bound each retained identity field independently; no arbitrary metadata map.
pub const PROVIDER_IDENTITY_FIELD_BYTES: usize = 256;

/// Provider cost in billionths of an OpenRouter credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCost {
	pub nanocredits: i64,
	pub upstream_nanocredits: Option<i64>,
}

impl ProviderCost {
	/// Read `cost` and the optional upstream cost from the provider's raw JSON
	/// text. Parsing through `f64` first would round digits away before the
	/// conversion. The cost is `None` unless `cost` is a valid non-negative JSON
	/// number.
	pub fn from_report(cost: Option<&str>, upstream: Option<&str>) -> Option<Self> {
		Some(Self {
			nanocredits: cost.and_then(parse_nanocredits)?,
			upstream_nanocredits: upstream.and_then(parse_nanocredits),
		})
	}
}

/// Convert raw JSON number text in credits to nanocredits without binary
/// floating-point arithmetic. Digits past nine decimals round up, so a recorded
/// cost never understates the report. Parse
/// `-?digits(.digits)?([eE][+-]?digits)?`; negative and out-of-range amounts
/// and any other JSON value are rejected.
pub fn parse_nanocredits(text: &str) -> Option<i64> {
	const SCALE: i64 = 9;
	const MAX_DIGITS: i64 = 19;
	let (mantissa, exponent) = match text.split_once(['e', 'E']) {
		Some((mantissa, exponent)) => (mantissa, parse_exponent(exponent)?),
		None => (text, 0),
	};
	let (negative, mantissa) = match mantissa.strip_prefix('-') {
		Some(rest) => (true, rest),
		None => (false, mantissa),
	};
	let (whole, fraction) = match mantissa.split_once('.') {
		Some((whole, fraction)) if !fraction.is_empty() => (whole, fraction),
		Some(_) => return None,
		None => (mantissa, ""),
	};
	if whole.is_empty()
		|| !whole
			.bytes()
			.chain(fraction.bytes())
			.all(|b| b.is_ascii_digit())
	{
		return None;
	}
	let digits = format!("{whole}{fraction}");
	let digits = digits.trim_start_matches('0');
	if digits.is_empty() {
		return Some(0);
	}
	if negative {
		return None;
	}
	// The amount equals `digits * 10^shift` nanocredits.
	let shift = exponent - fraction.len() as i64 + SCALE;
	let length = digits.len() as i64;
	if length + shift > MAX_DIGITS {
		return None;
	}
	let (kept, dropped) = if shift >= 0 {
		(digits, "")
	} else if length + shift <= 0 {
		("", digits)
	} else {
		digits.split_at((length + shift) as usize)
	};
	let mut amount: i64 = 0;
	for digit in kept.bytes() {
		amount = amount
			.checked_mul(10)?
			.checked_add(i64::from(digit - b'0'))?;
	}
	for _ in 0..shift.max(0) {
		amount = amount.checked_mul(10)?;
	}
	if dropped.bytes().any(|digit| digit != b'0') {
		amount = amount.checked_add(1)?;
	}
	Some(amount)
}

/// Saturate far beyond any representable shift so huge exponents stay exact.
fn parse_exponent(text: &str) -> Option<i64> {
	const BOUND: i64 = 1_000_000;
	let (negative, digits) = match text.as_bytes().first() {
		Some(b'-') => (true, &text[1..]),
		Some(b'+') => (false, &text[1..]),
		_ => (false, text),
	};
	if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
		return None;
	}
	let magnitude = digits.bytes().fold(0_i64, |value, digit| {
		(value * 10 + i64::from(digit - b'0')).min(BOUND)
	});
	Some(if negative { -magnitude } else { magnitude })
}

/// How far Aidash trusts its own request estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateConfidence {
	/// Counted with a Token Encoding: still an upper bound, never the
	/// provider's count, since templates add tokens no encoding sees.
	Tokenized,
	Calibrated,
	Conservative,
}

impl EstimateConfidence {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Tokenized => "tokenized",
			Self::Calibrated => "calibrated",
			Self::Conservative => "conservative",
		}
	}
}

/// Aidash's estimate of the request size. It is labeled as an estimate and is
/// never merged into reported counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEstimate {
	/// The whole Request Estimate, output reservation included.
	pub tokens: u64,
	pub estimator: String,
	pub version: u32,
	pub confidence: EstimateConfidence,
	/// Serialized request body plus the cache-salt reserve, in bytes. Every
	/// attempt records it, whichever estimator it used, so it can calibrate.
	pub byte_basis: u64,
	/// The output reservation inside `tokens`; `tokens - output_reservation`
	/// is the input-side estimate compared with reported prompt tokens.
	pub output_reservation: u64,
	/// The Estimate Snapshot's 99th-percentile ratio in parts per million;
	/// `None` for the byte estimator.
	pub ratio_ppm: Option<u32>,
}

/// A prompt-free measurement using a separately pinned estimator snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowEstimate {
	pub snapshot: super::estimate::EstimateSnapshot,
	pub estimate: RequestEstimate,
}

/// Why an attempt was dispatched; auxiliary work remains in task accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePurpose {
	Main,
	Summary,
}

impl UsagePurpose {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Main => "main",
			Self::Summary => "summary",
		}
	}
}

/// Facts recorded before provider I/O for one Inference Attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageDispatch {
	pub purpose: UsagePurpose,
	/// Public Inference Attempt identifier; never the worker lease token.
	pub attempt: Uuid,
	/// Worker lease that dispatched the attempt.
	pub lease: Uuid,
	pub response_epoch: i64,
	pub model_id: String,
	pub model_version: String,
	/// The Run's pinned Projection Version, as [`crate::projection::ProjectionVersion::number`].
	pub projection_version: u8,
	/// The Run's pinned Harness Contract, as [`crate::harness_contract::HarnessContract::number`].
	pub harness_contract: u8,
	/// The upstream model slug (`ModelConfig::model_id`), the calibration key.
	pub upstream_model: String,
	pub estimate: RequestEstimate,
	/// Requested exact upstream tag, distinct from provider-reported identity.
	pub requested_route: Option<String>,
	pub shadow_estimates: Vec<ShadowEstimate>,
	/// A complete outcome becomes a calibration sample: a media-free Inference
	/// Attempt of a Run step (ADR 0017).
	pub calibration_sample: bool,
	/// The request's Stable Prefix cache identity
	/// ([`crate::provider::ModelRequest::cache_identity`]); `None` for an
	/// unsalted request, which has no Cache Scope.
	pub cache_identity: Option<String>,
}

/// Terminal outcome of a dispatched Inference Attempt. Interrupted, timed-out
/// and crashed attempts are `Unknown` and carry no counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageOutcome {
	Completed(ReportedUsage),
	Rejected { class: String },
	Unknown,
}

impl UsageOutcome {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Completed(_) => "completed",
			Self::Rejected { .. } => "rejected",
			Self::Unknown => "unknown",
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;
	use serde_json::json;

	#[rstest]
	#[case::integer("2", Some(2_000_000_000))]
	#[case::zero("0", Some(0))]
	#[case::zero_fraction("0.000", Some(0))]
	#[case::negative_zero("-0.0", Some(0))]
	#[case::nine_decimals("0.000000001", Some(1))]
	#[case::exact_decimal("0.0001234", Some(123_400))]
	#[case::rounds_up_past_nine_decimals("0.0000000011", Some(2))]
	#[case::rounds_up_below_one_nanocredit("0.0000000000001", Some(1))]
	#[case::trailing_zero_digits_do_not_round("0.0000000010000", Some(1))]
	#[case::not_binary_float("0.3", Some(300_000_000))]
	#[case::exponent("1.5e-7", Some(150))]
	#[case::upper_exponent("1E-9", Some(1))]
	#[case::positive_exponent("2.5e+2", Some(250_000_000_000))]
	#[case::exponent_rounds_up("1.0000000001e-1", Some(100_000_001))]
	#[case::maximum("9223372036.854775807", Some(i64::MAX))]
	#[case::overflow("9223372036.854775808", None)]
	#[case::rounding_overflow("9223372036.8547758071", None)]
	#[case::huge_negative_exponent("5e-999999999999", Some(1))]
	#[case::huge_exponent("1e300", None)]
	#[case::negative("-0.5", None)]
	#[case::empty_fraction("1.", None)]
	#[case::empty_whole(".5", None)]
	#[case::empty_exponent("1e", None)]
	#[case::not_a_number("abc", None)]
	fn nanocredits_parse_decimal_text_exactly(#[case] text: &str, #[case] expected: Option<i64>) {
		assert_eq!(parse_nanocredits(text), expected);
	}

	#[rstest]
	#[case::string("\"0.1\"")]
	#[case::null("null")]
	#[case::boolean("true")]
	fn non_number_json_text_is_not_a_cost(#[case] text: &str) {
		assert_eq!(parse_nanocredits(text), None);
	}

	#[rstest]
	#[case::both(Some("0.002"), Some("0.0015"), Some(ProviderCost { nanocredits: 2_000_000, upstream_nanocredits: Some(1_500_000) }))]
	#[case::cost_only(Some("0.002"), None, Some(ProviderCost { nanocredits: 2_000_000, upstream_nanocredits: None }))]
	#[case::upstream_only(None, Some("0.0015"), None)]
	#[case::invalid_cost(Some("\"free\""), Some("0.0015"), None)]
	#[case::beyond_f64_precision(Some("0.1234567890000000000000000001"), None, Some(ProviderCost { nanocredits: 123_456_790, upstream_nanocredits: None }))]
	fn provider_cost_requires_a_valid_cost(
		#[case] cost: Option<&str>,
		#[case] upstream: Option<&str>,
		#[case] expected: Option<ProviderCost>,
	) {
		assert_eq!(ProviderCost::from_report(cost, upstream), expected);
	}

	#[rstest]
	fn pending_responses_keep_the_previous_json_shape() {
		let previous = json!({"text":"done","tool_calls":[],"input_tokens":3,"output_tokens":5,"usage_complete":true});
		let response = crate::provider::ModelResponse {
			text: "done".into(),
			input_tokens: 3,
			output_tokens: 5,
			usage_complete: true,
			reported: ReportedUsage {
				input_tokens: Some(3),
				output_tokens: Some(5),
				cache_read_tokens: Some(2),
				..Default::default()
			},
			..Default::default()
		};
		assert_eq!(serde_json::to_value(&response).unwrap(), previous);
		let restored: crate::provider::ModelResponse = serde_json::from_value(previous).unwrap();
		assert_eq!(restored.reported, ReportedUsage::default());
	}
}
