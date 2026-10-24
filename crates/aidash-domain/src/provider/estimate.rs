//! Request Estimates (ADR 0017). The byte estimate is the upper bound. A
//! calibrated estimate scales only its byte basis, by a ratio of reported
//! prompt tokens to bytes that one Run step pins in its Estimate Snapshot. A
//! tokenized estimate counts the model-visible text with the Token Encoding
//! the model definition names, and keeps the ratio only for byte quotas.
use super::usage::{EstimateConfidence, RequestEstimate};
pub use encoding::TokenEncoding;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Latest samples per calibration key that the ratio is taken over.
pub const CALIBRATION_WINDOW: usize = 200;
/// A key with fewer usable samples stays on the byte estimate.
pub const CALIBRATION_MINIMUM: usize = 20;
/// Safety margin added to the 99th-percentile ratio, in thousandths.
pub const CALIBRATION_MARGIN_PERMILLE: u32 = 100;
/// Ratios are fixed-point parts per million, so snapshots stay exact in JSON.
pub const PPM: u64 = 1_000_000;
/// Token reserve for provider message framing, chat templates and tool-use
/// preambles that are not part of the serialized body.
pub const FRAMING_RESERVE: usize = 1024;

const BYTES_VERSION: u32 = 1;
const CALIBRATED_VERSION: u32 = 1;
const TOKENIZED_VERSION: u32 = 1;

pub mod encoding;

/// The estimator a model definition opts into. Omitted means `bytes`, so
/// existing definition versions keep their fitting and compaction timing.
/// `{"tokenized": "<encoding>"}` names a Token Encoding; an unlisted name is
/// rejected when the definition is read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestEstimator {
	#[default]
	Bytes,
	Calibrated,
	Tokenized(TokenEncoding),
}

/// The byte estimate split by unit. Only `basis` is text the provider
/// tokenizes; the other parts are already token allowances.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ByteEstimate {
	/// Serialized request body plus the cache-salt reserve.
	pub basis: usize,
	/// Bounded provider-side media allowance.
	pub media: usize,
	/// Output reservation (`max_output_tokens`).
	pub output: usize,
	pub framing: usize,
	/// The same basis counted by a tokenized snapshot's Token Encoding, with
	/// its message and tool framing and the cache-salt reserve. `None` for
	/// every other snapshot, or when the encoding is unavailable.
	pub tokenized: Option<usize>,
}

impl ByteEstimate {
	/// The conservative estimate: one token per byte of basis.
	pub fn total(&self) -> usize {
		self.basis
			.saturating_add(self.media)
			.saturating_add(self.output)
			.saturating_add(self.framing)
	}
}

/// One complete, media-free Inference Attempt of a Run step: the provider's
/// reported prompt tokens for a request with this byte basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationSample {
	pub byte_basis: u64,
	pub prompt_tokens: u64,
}

/// A calibration key's pinned ratio and the samples it was taken over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
	/// The 99th-percentile ratio of prompt tokens to byte basis, in parts per
	/// million, rounded up and capped at one token per byte.
	pub ratio_ppm: u32,
	pub samples: u32,
}

/// Calibrate over the first `CALIBRATION_WINDOW` samples (newest first).
/// `None` while fewer than `CALIBRATION_MINIMUM` of them are usable.
pub fn calibrate(samples: &[CalibrationSample]) -> Option<Calibration> {
	let mut ratios = samples
		.iter()
		.take(CALIBRATION_WINDOW)
		.filter(|sample| sample.byte_basis > 0 && sample.prompt_tokens > 0)
		.map(|sample| {
			sample
				.prompt_tokens
				.saturating_mul(PPM)
				.div_ceil(sample.byte_basis)
				.min(PPM)
		})
		.collect::<Vec<_>>();
	if ratios.len() < CALIBRATION_MINIMUM {
		return None;
	}
	ratios.sort_unstable();
	// Nearest rank: the smallest ratio at or above 99% of the samples.
	let rank = (ratios.len() * 99).div_ceil(100);
	Some(Calibration {
		ratio_ppm: u32::try_from(ratios[rank - 1]).ok()?,
		samples: u32::try_from(ratios.len()).ok()?,
	})
}

/// How an Estimate Snapshot turns a byte estimate into tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "id", rename_all = "snake_case", deny_unknown_fields)]
pub enum Estimator {
	/// One token per byte of basis: today's conservative estimate.
	Bytes { version: u32 },
	/// The pinned 99th-percentile ratio plus margin, capped at bytes.
	Calibrated {
		version: u32,
		ratio_ppm: u32,
		margin_permille: u32,
		samples: u32,
	},
	/// The basis counted with a Token Encoding, capped at bytes. Byte quotas
	/// still convert at the key's calibrated ratio, or one token per byte
	/// when the key had none.
	Tokenized {
		version: u32,
		/// `None` when the pinned name is no longer listed.
		#[serde(deserialize_with = "encoding::lenient")]
		#[schemars(with = "Option<TokenEncoding>")]
		encoding: Option<TokenEncoding>,
		encoding_version: u32,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		ratio_ppm: Option<u32>,
		margin_permille: u32,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		samples: Option<u32>,
	},
}

/// The estimator, version, ratio and margin one Run step uses for all its
/// Request Estimates. It is fixed before the step first relies on an estimate
/// and reused when the step resumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EstimateSnapshot {
	/// The Run step that pinned this snapshot.
	pub step: i32,
	pub estimator: Estimator,
}

impl Default for EstimateSnapshot {
	fn default() -> Self {
		Self::conservative(0)
	}
}

impl EstimateSnapshot {
	pub fn conservative(step: i32) -> Self {
		Self {
			step,
			estimator: Estimator::Bytes {
				version: BYTES_VERSION,
			},
		}
	}

	pub fn calibrated(step: i32, ratio_ppm: u32, samples: u32) -> Self {
		Self {
			step,
			estimator: Estimator::Calibrated {
				version: CALIBRATED_VERSION,
				ratio_ppm,
				margin_permille: CALIBRATION_MARGIN_PERMILLE,
				samples,
			},
		}
	}

	/// A tokenized snapshot. Its byte quotas use `calibration` when the key
	/// has one.
	pub fn tokenized(step: i32, encoding: TokenEncoding, calibration: Option<Calibration>) -> Self {
		Self {
			step,
			estimator: Estimator::Tokenized {
				version: TOKENIZED_VERSION,
				encoding: Some(encoding),
				encoding_version: encoding.version(),
				ratio_ppm: calibration.map(|calibration| calibration.ratio_ppm),
				margin_permille: CALIBRATION_MARGIN_PERMILLE,
				samples: calibration.map(|calibration| calibration.samples),
			},
		}
	}

	/// The snapshot a new step pins. A demoted Run is conservative. Otherwise
	/// a calibrated model needs enough samples, and a tokenized model needs
	/// its encoding in this binary; it calibrates only its byte quotas.
	pub fn choose(
		step: i32,
		estimator: RequestEstimator,
		demoted: bool,
		samples: &[CalibrationSample],
	) -> Self {
		if demoted {
			return Self::conservative(step);
		}
		match estimator {
			RequestEstimator::Calibrated => match calibrate(samples) {
				Some(calibration) => {
					Self::calibrated(step, calibration.ratio_ppm, calibration.samples)
				}
				None => Self::conservative(step),
			},
			RequestEstimator::Tokenized(encoding) if encoding.available() => {
				Self::tokenized(step, encoding, calibrate(samples))
			}
			RequestEstimator::Tokenized(_) | RequestEstimator::Bytes => Self::conservative(step),
		}
	}

	/// Whether this is the byte estimate, which a Context Overflow or an
	/// underestimate cannot demote any further.
	pub fn is_conservative(&self) -> bool {
		matches!(self.estimator, Estimator::Bytes { .. })
	}

	/// The Token Encoding this snapshot counts with: the pinned one, at its
	/// pinned version, when this binary has it. A tokenized snapshot without
	/// one cannot be used (ADR 0017).
	pub fn encoding(&self) -> Option<TokenEncoding> {
		match self.estimator {
			Estimator::Tokenized {
				encoding: Some(encoding),
				encoding_version,
				..
			} if encoding_version == encoding.version() && encoding.available() => Some(encoding),
			_ => None,
		}
	}

	/// Whether this binary can estimate with the snapshot. Only a tokenized
	/// snapshot whose encoding is missing cannot; its step runs on bytes.
	pub fn usable(&self) -> bool {
		!matches!(self.estimator, Estimator::Tokenized { .. }) || self.encoding().is_some()
	}

	pub fn id(&self) -> &'static str {
		match self.estimator {
			Estimator::Bytes { .. } => "bytes",
			Estimator::Calibrated { .. } => "calibrated",
			Estimator::Tokenized { .. } => "tokenized",
		}
	}

	pub fn version(&self) -> u32 {
		match self.estimator {
			Estimator::Bytes { version }
			| Estimator::Calibrated { version, .. }
			| Estimator::Tokenized { version, .. } => version,
		}
	}

	pub fn confidence(&self) -> EstimateConfidence {
		match self.estimator {
			Estimator::Bytes { .. } => EstimateConfidence::Conservative,
			Estimator::Calibrated { .. } => EstimateConfidence::Calibrated,
			Estimator::Tokenized { .. } => EstimateConfidence::Tokenized,
		}
	}

	/// The pinned 99th-percentile ratio, without margin; `None` for bytes and
	/// for a tokenized snapshot of a key without calibration.
	pub fn ratio_ppm(&self) -> Option<u32> {
		match self.estimator {
			Estimator::Bytes { .. } => None,
			Estimator::Calibrated { ratio_ppm, .. } => Some(ratio_ppm),
			Estimator::Tokenized { ratio_ppm, .. } => ratio_ppm,
		}
	}

	fn margin_permille(&self) -> Option<u32> {
		match self.estimator {
			Estimator::Bytes { .. } => None,
			Estimator::Calibrated {
				margin_permille, ..
			} => Some(margin_permille),
			Estimator::Tokenized {
				ratio_ppm,
				margin_permille,
				..
			} => ratio_ppm.map(|_| margin_permille),
		}
	}

	/// The snapshot that sizes byte quotas and context growth: a tokenized
	/// snapshot's calibrated ratio, or bytes. Counting every probe would
	/// tokenize the whole context per tool event, and quotas are only an
	/// advance allocation; the complete request is counted (ADR 0017).
	pub fn quota(&self) -> Self {
		match self.estimator {
			Estimator::Tokenized {
				ratio_ppm: Some(ratio_ppm),
				samples,
				..
			} => Self::calibrated(self.step, ratio_ppm, samples.unwrap_or_default()),
			Estimator::Tokenized { .. } => Self::conservative(self.step),
			Estimator::Bytes { .. } | Estimator::Calibrated { .. } => *self,
		}
	}

	/// Tokens per byte of basis, margin included, never above one.
	fn scale_ppm(&self) -> u64 {
		match (self.ratio_ppm(), self.margin_permille()) {
			(Some(ratio_ppm), Some(margin_permille)) => u64::from(ratio_ppm)
				.saturating_mul(1000 + u64::from(margin_permille))
				.div_ceil(1000)
				.clamp(1, PPM),
			_ => PPM,
		}
	}

	/// Tokens for `bytes` of basis, rounded up.
	pub fn tokens_for_bytes(&self, bytes: usize) -> usize {
		let tokens = (bytes as u64)
			.saturating_mul(self.scale_ppm())
			.div_ceil(PPM);
		usize::try_from(tokens).unwrap_or(usize::MAX)
	}

	/// Bytes of basis that fit in `tokens`, rounded down. Byte-denominated
	/// quotas convert through this, so they never exceed the token budget.
	pub fn bytes_for_tokens(&self, tokens: usize) -> usize {
		let bytes = (tokens as u64).saturating_mul(PPM) / self.scale_ppm();
		usize::try_from(bytes).unwrap_or(usize::MAX)
	}

	/// The Request Estimate in tokens, capped at the byte estimate.
	pub fn tokens(&self, bytes: &ByteEstimate) -> usize {
		let basis = match (self.estimator, bytes.tokenized) {
			(Estimator::Tokenized { .. }, Some(tokenized)) => tokenized,
			_ => self.tokens_for_bytes(bytes.basis),
		};
		basis
			.saturating_add(bytes.media)
			.saturating_add(bytes.output)
			.saturating_add(bytes.framing)
			.min(bytes.total())
	}

	/// The labeled Request Estimate recorded with an Inference Attempt.
	pub fn request_estimate(&self, bytes: &ByteEstimate) -> RequestEstimate {
		RequestEstimate {
			tokens: self.tokens(bytes) as u64,
			estimator: self.id().into(),
			version: self.version(),
			confidence: self.confidence(),
			byte_basis: bytes.basis as u64,
			output_reservation: bytes.output as u64,
			ratio_ppm: self.ratio_ppm(),
		}
	}
}

/// A Run's Estimate Snapshots, kept in its context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EstimateState {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub snapshot: Option<EstimateSnapshot>,
	/// Set once a Context Overflow followed a calibrated estimate. Every later
	/// snapshot of the Run is conservative.
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub demoted: bool,
}

impl EstimateState {
	pub fn is_initial(&self) -> bool {
		*self == Self::default()
	}

	/// The snapshot already pinned for `step`, if any.
	pub fn pinned(&self, step: i32) -> Option<EstimateSnapshot> {
		self.snapshot.filter(|snapshot| snapshot.step == step)
	}

	/// The latest pinned snapshot, conservative before the first one. Tool
	/// results recorded after a response use the snapshot of its request.
	pub fn current(&self) -> EstimateSnapshot {
		self.snapshot.unwrap_or_default()
	}

	/// Demote a calibrated or tokenized snapshot after a Context Overflow: the
	/// step and every later step of the Run use bytes. Returns whether
	/// anything changed.
	pub fn demote(&mut self) -> bool {
		match self.snapshot {
			Some(snapshot) if !snapshot.is_conservative() => {
				self.snapshot = Some(EstimateSnapshot::conservative(snapshot.step));
				self.demoted = true;
				true
			}
			_ => false,
		}
	}

	/// Demote the Run from its next step when the provider reported more input
	/// tokens than a calibrated or tokenized estimate predicted. The step's
	/// own snapshot stays, since its request was already sent. This is how a
	/// wrong or outdated Token Encoding is noticed. Returns whether the Run
	/// was demoted now.
	pub fn underestimated(&mut self, estimate: &RequestEstimate, reported_input: u64) -> bool {
		let input = estimate.tokens.saturating_sub(estimate.output_reservation);
		if self.demoted || self.current().is_conservative() || reported_input <= input {
			return false;
		}
		self.demoted = true;
		true
	}
}

/// Run inspection's view of the pinned Estimate Snapshot. It omits the sample
/// count: on a shared Deployment, its growth reveals other Tenants' use of
/// the same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EstimateView {
	pub step: i32,
	pub estimator: &'static str,
	pub version: u32,
	/// The Token Encoding a tokenized snapshot pinned.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub encoding: Option<TokenEncoding>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub ratio_ppm: Option<u32>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub margin_permille: Option<u32>,
	#[serde(skip_serializing_if = "std::ops::Not::not")]
	pub demoted: bool,
}

impl EstimateState {
	pub fn view(&self) -> Option<EstimateView> {
		let snapshot = self.snapshot?;
		let encoding = match snapshot.estimator {
			Estimator::Tokenized { encoding, .. } => encoding,
			Estimator::Bytes { .. } | Estimator::Calibrated { .. } => None,
		};
		Some(EstimateView {
			step: snapshot.step,
			estimator: snapshot.id(),
			version: snapshot.version(),
			encoding,
			ratio_ppm: snapshot.ratio_ppm(),
			margin_permille: snapshot.margin_permille(),
			demoted: self.demoted,
		})
	}
}

#[cfg(test)]
mod tests;
