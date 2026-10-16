//! The sweeper that purges Document snapshot contents without a retention
//! basis (ADR 0045). The current periods apply to everything still kept.
use crate::{
	Result,
	ports::research::{RetainedSnapshot, SnapshotRetention},
};
use aidash_domain::retention::{RetentionRules, SnapshotPeriods};
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
	// A snapshot keeps its basis until the shortest maximum age any Tenant
	// may apply, and a snapshot of an ended Run until the shortest post-run
	// period; nothing younger is even examined.
	let periods = deployment.document_snapshots;
	let shortest = port
		.shortest_tenant_periods()
		.await?
		.map_or(periods, |tenant| SnapshotPeriods {
			after_run_seconds: tenant.after_run_seconds.min(periods.after_run_seconds),
			max_age_seconds: tenant.max_age_seconds.min(periods.max_age_seconds),
		});
	let retrieved_before = now - Duration::seconds(i64::from(shortest.max_age_seconds));
	let ended_before = now - Duration::seconds(i64::from(shortest.after_run_seconds));
	let expired = |candidate: &RetainedSnapshot| {
		!deployment
			.effective(candidate.tenant_rules.as_ref())
			.document_snapshots
			.retains(candidate.fetched_at, candidate.run_ended_at, now)
	};
	let batch = batch.max(1);
	let mut after: Option<Uuid> = None;
	let mut purged = 0;
	loop {
		let candidates = port
			.candidates(after, retrieved_before, ended_before, batch)
			.await?;
		let Some(last) = candidates.last() else {
			break;
		};
		after = Some(last.id);
		let ids = candidates
			.iter()
			.filter(|candidate| expired(candidate))
			.map(|candidate| candidate.id)
			.collect::<Vec<_>>();
		// The purge decides again under the Tenants' current periods.
		if !ids.is_empty() {
			purged += port.purge(&ids, &expired, now).await?;
		}
		if candidates.len() < batch as usize {
			break;
		}
	}
	Ok(purged)
}

#[cfg(test)]
mod tests;
