//! The worker event determines which durable retry and diagnostic fields survive a save.
use super::encode;
use crate::{Result, Run};
use serde_json::Value;
/// Checkpoints that continue the same step and keep its worker lease.
pub fn retains_lease(event: &str) -> bool {
	matches!(
		event,
		"run.sources_observed"
			| "run.semantic_degraded"
			| "run.semantic_recovered"
			| "run.estimate_pinned"
			| "run.plugin_tools_pinned"
			| "run.mcp_checkpoint"
			| "context.summary_revoked"
	)
}
pub struct WorkerSnapshot<'a> {
	pub pending: Value,
	pub error: Option<&'a str>,
}
impl Run {
	pub fn worker_snapshot(&self, event: &str) -> Result<WorkerSnapshot<'_>> {
		let mut recovery = self.recovery.clone();
		let retrying = matches!(
			event,
			"run.retrying"
				| "run.semantic_retrying"
				| "run.decider_retrying"
				| "run.failure_pending"
		);
		if !retrying {
			recovery.retry = None;
		}
		// Only a scheduled Decider retry carries its count to the next lease.
		if event != "run.decider_retrying" {
			recovery.decider_retry = 0;
		}
		// A checkpoint inside the step keeps its Lease Recovery count, so a step
		// that crashes after a checkpoint still reaches the bound (#604). Only a
		// save that ends the step and releases the lease starts the count again.
		if !retains_lease(event) {
			recovery.lease_recoveries = 0;
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
