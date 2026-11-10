//! Durable read-loop tracking for Journal Query and its fallback.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A Journal range and the most recent projection entry that read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadRecord {
	pub from_seq: u64,
	pub through_seq: u64,
	pub result_seq: u64,
	pub protect_through_step: u32,
	pub reads: u32,
}

/// Durable state updated by the Journal Query or fallback after delivering a read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadGuard {
	pub ranges: Vec<ReadRecord>,
	#[serde(default)]
	pub summary_pending: bool,
	/// Kept until an Agent inference receives the repeated-read notice.
	#[serde(default)]
	pub notice_pending: bool,
}

impl ReadGuard {
	pub fn is_empty(&self) -> bool {
		self.ranges.is_empty() && !self.summary_pending && !self.notice_pending
	}

	/// Record a delivered read. `true` requests a Summary Stage and a notice
	/// once the same range has been read `threshold` times.
	pub fn record(
		&mut self,
		from_seq: u64,
		through_seq: u64,
		result_seq: u64,
		step: u32,
		immunity: u32,
		threshold: u32,
	) -> bool {
		if from_seq == 0 || through_seq < from_seq || result_seq == 0 {
			return false;
		}
		let reads = self
			.ranges
			.iter()
			.filter(|item| item.from_seq == from_seq && item.through_seq == through_seq)
			.map(|item| item.reads)
			.max()
			.unwrap_or_default()
			.saturating_add(1);
		self.ranges.push(ReadRecord {
			from_seq,
			through_seq,
			result_seq,
			protect_through_step: step.saturating_add(immunity),
			reads,
		});
		if reads >= threshold {
			self.summary_pending = true;
			self.notice_pending = true;
		}
		self.summary_pending
	}

	pub fn protects(&self, result_seq: u64, step: u32) -> bool {
		self.ranges
			.iter()
			.any(|item| item.result_seq == result_seq && step <= item.protect_through_step)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn repeated_range_is_protected_then_requests_summary() {
		let mut guard = ReadGuard::default();
		assert!(!guard.record(3, 5, 10, 20, 2, 3));
		assert!(guard.protects(10, 22));
		assert!(!guard.protects(10, 23));
		assert!(!guard.record(3, 5, 11, 23, 2, 3));
		assert!(guard.record(3, 5, 12, 24, 2, 3));
		assert!(guard.protects(12, 26));
		assert!(guard.notice_pending);
		guard.summary_pending = false;
		assert!(
			guard.notice_pending,
			"the next Agent inference still needs the notice"
		);
	}
}
