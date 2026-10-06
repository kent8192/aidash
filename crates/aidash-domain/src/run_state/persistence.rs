//! The worker event determines which durable retry and diagnostic fields survive a save.
use super::encode;
use crate::{Result, Run};
use serde_json::Value;
pub struct WorkerSnapshot<'a> {
	pub pending: Value,
	pub error: Option<&'a str>,
}
impl Run {
	pub fn worker_snapshot(&self, event: &str) -> Result<WorkerSnapshot<'_>> {
		let mut recovery = self.recovery.clone();
		let retrying = matches!(
			event,
			"run.retrying" | "run.semantic_retrying" | "run.failure_pending"
		);
		if !retrying {
			recovery.retry = None;
		}
		let pending = encode(&self.state, &recovery)?;
		let error = if retrying || event == "run.failed" {
			self.error.as_deref()
		} else {
			None
		};
		Ok(WorkerSnapshot { pending, error })
	}
}
#[cfg(test)]
mod tests;
