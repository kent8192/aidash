//! The compaction owner supplies ranges offloaded at a Run step. Extraction
//! is enqueued separately so the foreground inference never waits for it.
use crate::{Error, Result};
use aidash_domain::memory::Evidence;
use async_trait::async_trait;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalRange {
	pub run: Uuid,
	pub from: u64,
	pub through: u64,
}

impl JournalRange {
	pub fn evidence(self) -> Result<Evidence> {
		if self.run.is_nil()
			|| self.from == 0
			|| self.from > self.through
			|| self.through > i64::MAX as u64
		{
			return Err(Error::Invalid("invalid offloaded Journal range".into()));
		}
		Ok(Evidence::JournalRange {
			run_id: self.run,
			from_seq: self.from,
			through_seq: self.through,
		})
	}
}

/// Stream 1's eventual compaction adapter provides the durable ranges.
#[async_trait]
pub trait OffloadedRanges: Send + Sync {
	async fn at_step(&self, run: Uuid, step: u32) -> Result<Vec<JournalRange>>;
}

/// Persists background work; implementations deduplicate a Run and exact range.
#[async_trait]
pub trait BackgroundExtraction: Send + Sync {
	async fn enqueue(&self, range: JournalRange) -> Result<()>;
}

/// Until compaction supplies the ranges, no offload can be inferred safely.
pub struct NoOffloadedRanges;

#[async_trait]
impl OffloadedRanges for NoOffloadedRanges {
	async fn at_step(&self, _run: Uuid, _step: u32) -> Result<Vec<JournalRange>> {
		Ok(Vec::new())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn rejects_invalid_ranges_before_scheduling() {
		let run = Uuid::now_v7();
		assert!(
			JournalRange {
				run,
				from: 2,
				through: 1
			}
			.evidence()
			.is_err()
		);
		assert!(
			JournalRange {
				run,
				from: 1,
				through: i64::MAX as u64 + 1
			}
			.evidence()
			.is_err()
		);
		assert_eq!(
			JournalRange {
				run,
				from: 1,
				through: 3
			}
			.evidence()
			.unwrap(),
			Evidence::JournalRange {
				run_id: run,
				from_seq: 1,
				through_seq: 3
			}
		);
	}
}
