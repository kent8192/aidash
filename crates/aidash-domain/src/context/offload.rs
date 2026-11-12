//! Durable read-loop tracking for Journal Query and its fallback.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A Journal range and the most recent projection entry that read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadRecord {
	/// None identifies the Run whose context contains this guard.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub target_run: Option<uuid::Uuid>,
	pub from_seq: u64,
	pub through_seq: u64,
	/// Character offset of the delivered page. Paging through one range is
	/// not reading it again.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub offset: u64,
	pub result_seq: u64,
	pub protect_through_step: u32,
	/// Zero for a read that only protects its result, such as a search or
	/// query without a range, which never counts as reading a range again.
	pub reads: u32,
}

fn is_zero(value: &u64) -> bool {
	*value == 0
}

/// One delivered page of a Journal range: the range and the character
/// offset the page starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
	pub from_seq: u64,
	pub through_seq: u64,
	pub offset: u64,
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
		self.record_for(
			None,
			Page {
				from_seq,
				through_seq,
				offset: 0,
			},
			result_seq,
			step,
			immunity,
			threshold,
		)
	}

	/// Record a delivered page of a particular Run using the same durable
	/// guard. Only a repeat of the same page counts toward `threshold`.
	pub fn record_for(
		&mut self,
		target_run: Option<uuid::Uuid>,
		page: Page,
		result_seq: u64,
		step: u32,
		immunity: u32,
		threshold: u32,
	) -> bool {
		let Page {
			from_seq,
			through_seq,
			offset,
		} = page;
		if from_seq == 0 || through_seq < from_seq || result_seq == 0 {
			return false;
		}
		let same = |item: &ReadRecord| {
			item.reads > 0
				&& item.target_run == target_run
				&& item.from_seq == from_seq
				&& item.through_seq == through_seq
				&& item.offset == offset
		};
		let reads = self
			.ranges
			.iter()
			.filter(|item| same(item))
			.map(|item| item.reads)
			.max()
			.unwrap_or_default()
			.saturating_add(1);
		self.push(
			ReadRecord {
				target_run,
				from_seq,
				through_seq,
				offset,
				result_seq,
				protect_through_step: step.saturating_add(immunity),
				reads,
			},
			step,
		);
		if reads >= threshold {
			self.summary_pending = true;
			self.notice_pending = true;
		}
		self.summary_pending
	}

	/// Keep a delivered result out of offloading during its immunity without
	/// counting it as a read of any range.
	pub fn protect(
		&mut self,
		target_run: Option<uuid::Uuid>,
		result_seq: u64,
		step: u32,
		immunity: u32,
	) {
		if result_seq == 0 {
			return;
		}
		self.push(
			ReadRecord {
				target_run,
				from_seq: 1,
				through_seq: i64::MAX as u64,
				offset: 0,
				result_seq,
				protect_through_step: step.saturating_add(immunity),
				reads: 0,
			},
			step,
		);
	}

	/// Add `record` and drop records whose protection ended before its step
	/// and that no longer carry a count: a read that only protected its
	/// result, or an earlier read of a page whose newer record carries the
	/// count. The guard then grows with distinct pages, not with every read.
	fn push(&mut self, record: ReadRecord, step: u32) {
		let superseded = |item: &ReadRecord| {
			item.reads == 0
				|| (record.reads > 0
					&& item.target_run == record.target_run
					&& item.from_seq == record.from_seq
					&& item.through_seq == record.through_seq
					&& item.offset == record.offset)
		};
		self.ranges
			.retain(|item| item.protect_through_step >= step || !superseded(item));
		self.ranges.push(record);
	}

	/// Drop a summary request the Summary Stage could not satisfy while the
	/// request fits. Counting starts again, so only new repeated reads ask
	/// for another summary.
	pub fn release(&mut self) {
		self.summary_pending = false;
		for record in &mut self.ranges {
			record.reads = 0;
		}
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
	fn previously_persisted_read_records_default_to_own_run() {
		let guard: ReadGuard = serde_json::from_value(serde_json::json!({
			"ranges": [{"from_seq": 3, "through_seq": 5, "result_seq": 10,
				"protect_through_step": 22, "reads": 1}]
		}))
		.unwrap();
		assert_eq!(guard.ranges[0].target_run, None);
		assert!(guard.protects(10, 22));
	}

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

	fn page(offset: u64) -> Page {
		Page {
			from_seq: 3,
			through_seq: 5,
			offset,
		}
	}

	#[test]
	fn paging_through_one_range_is_not_reading_it_again() {
		let mut guard = ReadGuard::default();
		for (step, offset) in [(1, 0), (2, 4000), (3, 8000)] {
			assert!(!guard.record_for(None, page(offset), 10 + u64::from(step), step, 2, 3));
		}
		assert!(!guard.summary_pending && !guard.notice_pending);
		assert!(!guard.record_for(None, page(0), 20, 4, 2, 3));
		assert!(guard.record_for(None, page(0), 21, 5, 2, 3));
	}

	#[test]
	fn a_protect_only_read_never_counts_toward_a_summary() {
		let mut guard = ReadGuard::default();
		for step in 1..=5 {
			guard.protect(None, u64::from(step), step, 2);
		}
		assert!(!guard.summary_pending);
		assert!(guard.protects(5, 7));
		assert!(!guard.protects(1, 4));
	}

	#[test]
	fn records_whose_protection_ended_are_pruned() {
		let mut guard = ReadGuard::default();
		for step in 0..100 {
			guard.protect(None, u64::from(step) + 1, step, 2);
			guard.record_for(None, page(0), u64::from(step) + 1, step, 2, 1000);
		}
		// Only the reads still protected remain; the newest carries the count.
		assert!(guard.ranges.len() <= 8, "{}", guard.ranges.len());
		assert_eq!(guard.ranges.iter().map(|item| item.reads).max(), Some(100));
		assert!(guard.protects(100, 101));
	}

	#[test]
	fn release_drops_the_request_and_starts_counting_again() {
		let mut guard = ReadGuard::default();
		for step in 0..3 {
			guard.record(3, 5, u64::from(step) + 1, step, 2, 3);
		}
		assert!(guard.summary_pending);
		guard.release();
		assert!(!guard.summary_pending);
		assert!(!guard.record(3, 5, 10, 4, 2, 3));
		assert!(guard.protects(3, 4), "protection survives the release");
	}
}
