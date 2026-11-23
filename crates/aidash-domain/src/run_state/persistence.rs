//! The worker event determines which durable retry and diagnostic fields survive a save.
use super::encode;
use crate::{Result, Run, capabilities::sessions::QUEUED};
use serde_json::Value;
/// Checkpoints that continue the same step and keep its worker lease.
pub fn retains_lease(event: &str) -> bool {
	matches!(
		event,
		"execution.sources_observed"
			| "execution.semantic_degraded"
			| "execution.semantic_recovered"
			| "execution.estimate_pinned"
			| "execution.plugin_tools_pinned"
			| "execution.mcp_checkpoint"
			| "execution.unassigned_tasks_abandoned"
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
			"execution.retrying"
				| "execution.semantic_retrying"
				| "execution.decider_retrying"
				| "execution.node_retrying"
				| "execution.outage_waiting"
				| "execution.outage_resumed"
				| "execution.failure_pending"
		);
		if !retrying || event == "execution.outage_resumed" {
			recovery.retry = None;
		}
		// Only a scheduled Decider retry carries its count to the next lease.
		if !matches!(
			event,
			"execution.decider_retrying" | "execution.outage_waiting" | "execution.outage_resumed"
		) {
			recovery.decider_retry = 0;
		}
		// Likewise a scheduled Node retry (#751).
		if !matches!(
			event,
			"execution.node_retrying" | "execution.outage_waiting" | "execution.outage_resumed"
		) {
			recovery.node_retry = 0;
		}
		if !retrying && !retains_lease(event) {
			if recovery.outage.is_some() {
				recovery.semantic_reason = None;
				recovery.context_reason = None;
			}
			recovery.outage = None;
		}
		// A checkpoint inside the step keeps its Lease Recovery count, so a step
		// that crashes after a checkpoint still reaches the bound (#604). Only a
		// save that ends the step and releases the lease starts the count again.
		if !retains_lease(event) {
			recovery.lease_recoveries = 0;
		}
		let pending = encode(&self.state, &recovery)?;
		let error = if retrying || matches!(event, QUEUED | "execution.failed") {
			self.error.as_deref()
		} else {
			None
		};
		Ok(WorkerSnapshot { pending, error })
	}
}
#[cfg(test)]
mod tests;
