//! The sweeper that purges Document snapshot contents without a retention
//! basis (ADR 0045). The current periods apply to everything still kept.
use crate::{Result, ports::research::SnapshotRetention};
use aidash_domain::retention::RetentionRules;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

/// Snapshots examined per round trip.
pub const BATCH: u32 = 256;

/// Purge every snapshot whose contents lost their retention basis at `now`
/// under `deployment` and its Tenant's stored periods. Returns how many were
/// purged.
pub async fn sweep(
	port: &dyn SnapshotRetention,
	deployment: &RetentionRules,
	now: DateTime<Utc>,
	batch: u32,
) -> Result<u64> {
	// A snapshot of a Run that has not ended keeps its basis until the
	// shortest maximum age any Tenant may apply.
	let deployment_max_age = deployment.document_snapshots.max_age_seconds;
	let shortest = port
		.shortest_tenant_max_age()
		.await?
		.map_or(deployment_max_age, |tenant| tenant.min(deployment_max_age));
	let retrieved_before = now - Duration::seconds(i64::from(shortest));
	let batch = batch.max(1);
	let mut after: Option<Uuid> = None;
	let mut purged = 0;
	loop {
		let candidates = port.candidates(after, retrieved_before, batch).await?;
		let Some(last) = candidates.last() else {
			break;
		};
		after = Some(last.id);
		let expired = candidates
			.iter()
			.filter(|candidate| {
				!deployment
					.effective(candidate.tenant_rules.as_ref())
					.document_snapshots
					.retains(candidate.fetched_at, candidate.run_ended_at, now)
			})
			.map(|candidate| candidate.id)
			.collect::<Vec<_>>();
		if !expired.is_empty() {
			purged += port.purge(&expired, now).await?;
		}
		if candidates.len() < batch as usize {
			break;
		}
	}
	Ok(purged)
}

#[cfg(test)]
mod tests;
