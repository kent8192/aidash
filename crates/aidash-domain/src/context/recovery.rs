//! Typed context-recovery outcomes and the per-Run overflow allowance.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Why a Run paused instead of sending or acting on a provider request.
/// Each reason is final for its step; none of them is retried as transport.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error,
)]
#[schemars(rename = "ContextRecoveryFailure")]
#[serde(rename_all = "snake_case")]
pub enum Failure {
	#[error(
		"Mandatory context cannot fit the model window; reduce instructions, tools or pinned context"
	)]
	ContextUnreducible,
	#[error("Jev pruning is unavailable for this Context Policy")]
	PruneUnavailable,
	#[error("The context Summary Stage is unavailable or not approved for this Run")]
	SummaryUnavailable,
	#[error("The context summary was invalid or did not reduce the context")]
	SummaryInvalid,
	#[error("The provider still reported a context overflow after bounded recovery")]
	OverflowRetriesExhausted,
	#[error("The provider truncated its output; no tool call from it was dispatched")]
	OutputTruncated,
	#[error("The provider refused the request")]
	Refused,
}

impl Failure {
	/// Stable event and metric label.
	pub fn label(self) -> &'static str {
		match self {
			Self::ContextUnreducible => "context_unreducible",
			Self::PruneUnavailable => "prune_unavailable",
			Self::SummaryUnavailable => "summary_unavailable",
			Self::SummaryInvalid => "summary_invalid",
			Self::OverflowRetriesExhausted => "overflow_retries_exhausted",
			Self::OutputTruncated => "output_truncated",
			Self::Refused => "refused",
		}
	}
}

/// Per-Run state that bounds Context Overflow recovery. The effective window
/// shrink persists for the Run because its model binding is pinned; the
/// consecutive-overflow count resets after an accepted inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ContextRecoveryState")]
#[serde(deny_unknown_fields)]
pub struct RecoveryState {
	/// Effective window as thousandths of the configured request window.
	pub window_permille: u16,
	pub overflow_attempts: u8,
	/// Estimated request size of the last overflowing request.
	pub overflow_request_tokens: Option<u64>,
}

impl Default for RecoveryState {
	fn default() -> Self {
		Self {
			window_permille: 1000,
			overflow_attempts: 0,
			overflow_request_tokens: None,
		}
	}
}

impl RecoveryState {
	pub fn is_initial(&self) -> bool {
		*self == Self::default()
	}

	pub fn effective_window(&self, window: usize) -> usize {
		window.saturating_mul(usize::from(self.window_permille)) / 1000
	}

	/// Record a proven overflow and shrink the effective window. Returns false
	/// when the allowance is spent or the overflowing request did not shrink
	/// since the previous overflow, so recovery must pause instead of looping.
	pub fn overflowed(&mut self, request_tokens: u64, retries: u8, shrink_permille: u16) -> bool {
		if self.overflow_attempts >= retries
			|| self
				.overflow_request_tokens
				.is_some_and(|previous| request_tokens >= previous)
		{
			return false;
		}
		self.overflow_attempts += 1;
		self.overflow_request_tokens = Some(request_tokens);
		self.window_permille =
			(u32::from(self.window_permille) * u32::from(shrink_permille) / 1000) as u16;
		true
	}

	/// An accepted inference ends the current overflow episode.
	pub fn inference_accepted(&mut self) {
		self.overflow_attempts = 0;
		self.overflow_request_tokens = None;
	}
}

/// Pipeline stage that produced a Compaction Attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "CompactionStage")]
#[serde(rename_all = "snake_case")]
pub enum Stage {
	Prune,
	Summary,
}

impl Stage {
	pub fn label(self) -> &'static str {
		match self {
			Self::Prune => "prune",
			Self::Summary => "summary",
		}
	}
}

/// A Compaction Attempt recorded before provider I/O.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "CompactionAttempt")]
#[serde(deny_unknown_fields)]
pub struct Attempt {
	pub id: uuid::Uuid,
	pub run_id: uuid::Uuid,
	pub stage: Stage,
	pub policy_version: String,
	/// Exact provider pin, `id@version#definition-digest`.
	pub provider: String,
	pub source_from_seq: u64,
	pub source_through_seq: u64,
	pub base_revision: i64,
	pub observed_input_seq: i64,
	pub before_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "CompactionOutcome")]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Adopted,
	/// Valid, but the complete request still does not fit.
	Insufficient,
	/// Provider unavailable or the stage is not approved for this Run.
	Unavailable,
	/// Malformed, empty, dropped items, oversized or not reducing.
	Invalid,
	/// Authority changed before adoption.
	Unauthorized,
	/// Interrupted before settlement, found by a later attempt or lease recovery.
	Abandoned,
}

impl Outcome {
	pub fn label(self) -> &'static str {
		match self {
			Self::Adopted => "applied",
			Self::Insufficient => "insufficient",
			Self::Unavailable => "unavailable",
			Self::Invalid => "invalid",
			Self::Unauthorized => "unauthorized",
			Self::Abandoned => "abandoned",
		}
	}
}

/// How an Attempt ended. Only `Adopted` changes the saved Context Projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "CompactionSettlement")]
#[serde(deny_unknown_fields)]
pub struct Settlement {
	pub outcome: Outcome,
	/// Short machine label for invalid candidates, such as `dropped_item`.
	pub reason: Option<String>,
	pub candidate_digest: Option<String>,
	pub after_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn overflow_allowance_is_finite_and_requires_reduction() {
		let mut state = RecoveryState::default();
		assert!(state.overflowed(10_000, 2, 750));
		assert_eq!(state.effective_window(1000), 750);
		// A retry whose request did not shrink cannot consume another attempt.
		assert!(!state.overflowed(10_000, 2, 750));
		assert!(state.overflowed(7_000, 2, 750));
		assert_eq!(state.effective_window(1000), 562);
		assert!(!state.overflowed(5_000, 2, 750));
		state.inference_accepted();
		assert_eq!(state.effective_window(1000), 562);
		assert!(state.overflowed(5_000, 2, 750));
	}

	#[test]
	fn prune_only_policy_never_retries_overflow() {
		let mut state = RecoveryState::default();
		assert!(!state.overflowed(10_000, 0, 750));
		assert!(state.is_initial());
	}
}
