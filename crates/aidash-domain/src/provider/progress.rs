//! Tentative, display-only progress of one Inference Attempt.
//!
//! Inference Progress never carries partial tool arguments or provider
//! reasoning, and nothing in this module can become a `ModelResponse`. Only the
//! adapter's validated, complete response may enter ToolCall state.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Largest serialized progress item stored or delivered as one row or frame.
pub const MAX_PROGRESS_ITEM_BYTES: usize = 16 * 1024;
/// Buffered text is flushed once it reaches this size, even before the interval.
pub const FLUSH_BYTES: usize = 4 * 1024;
/// Longest time buffered progress waits before it is flushed.
pub const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// Unflushed progress beyond this bound is dropped and counted, never awaited.
pub const MAX_PENDING_BYTES: usize = 256 * 1024;
/// Progress rows are pruned this long after their attempt's outcome marker.
pub const RETENTION: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Public identity of one provider call for one Run step. It is distinct from
/// the worker lease token, which fences durable writes and is never disclosed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InferenceAttemptId(pub Uuid);

impl InferenceAttemptId {
	pub fn new() -> Self {
		Self(Uuid::now_v7())
	}
}

impl Default for InferenceAttemptId {
	fn default() -> Self {
		Self::new()
	}
}

impl std::fmt::Display for InferenceAttemptId {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

/// One sanitized progress item offered by a streaming adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InferenceProgress {
	/// Tentative assistant text. Consecutive items concatenate.
	Text { text: String },
	/// Assembly status of one tool call; arguments are reported only by size.
	ToolCall {
		index: u32,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		id: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		name: Option<String>,
		argument_bytes: u64,
	},
}

impl InferenceProgress {
	/// Bytes this item contributes to flush and pending bounds.
	pub fn weight(&self) -> usize {
		match self {
			Self::Text { text } => text.len(),
			Self::ToolCall { id, name, .. } => {
				32 + id.as_ref().map_or(0, String::len) + name.as_ref().map_or(0, String::len)
			}
		}
	}

	/// Merge `next` into `self` when both describe the same stream position:
	/// consecutive text, or newer status for the same tool call. Text merges stop
	/// at the per-item bound so stored rows stay within it.
	pub fn absorb(&mut self, next: &Self) -> bool {
		match (self, next) {
			(Self::Text { text }, Self::Text { text: more })
				if text.len() + more.len() <= MAX_PROGRESS_ITEM_BYTES / 2 =>
			{
				text.push_str(more);
				true
			}
			(
				Self::ToolCall {
					index,
					id,
					name,
					argument_bytes,
				},
				Self::ToolCall {
					index: next_index,
					id: next_id,
					name: next_name,
					argument_bytes: next_bytes,
				},
			) if index == next_index => {
				if next_id.is_some() {
					id.clone_from(next_id);
				}
				if next_name.is_some() {
					name.clone_from(next_name);
				}
				*argument_bytes = (*argument_bytes).max(*next_bytes);
				true
			}
			_ => false,
		}
	}
}

/// Why displayed progress ended without an Accepted Response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptionReason {
	Cancelled,
	LeaseLost,
	Stall,
	StreamError,
	Revoked,
}

impl InterruptionReason {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Cancelled => "cancelled",
			Self::LeaseLost => "lease_lost",
			Self::Stall => "stall",
			Self::StreamError => "stream_error",
			Self::Revoked => "revoked",
		}
	}
}

/// How an Inference Attempt's displayed progress ended. Written exactly once;
/// a later discard of an Accepted Response is a Run fact, not a new outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
pub enum ProgressOutcome {
	Accepted,
	/// New user input superseded the attempt.
	Discarded,
	Interrupted(InterruptionReason),
}

impl ProgressOutcome {
	/// Event kind of the low-rate marker recorded in the event journal.
	pub fn event_kind(self) -> &'static str {
		match self {
			Self::Accepted => "inference.accepted",
			Self::Discarded => "inference.discarded",
			Self::Interrupted(_) => "inference.interrupted",
		}
	}

	/// Metric and wire label: `accepted`, `correction`, or the interruption reason.
	pub fn reason(self) -> &'static str {
		match self {
			Self::Accepted => "accepted",
			Self::Discarded => "correction",
			Self::Interrupted(reason) => reason.as_str(),
		}
	}
}

/// Event kind recorded when an Inference Attempt starts.
pub const STARTED_EVENT: &str = "inference.started";

#[cfg(test)]
mod tests;
