//! Provider results under the Research account terms (ADR 0058): the sweeper
//! that purges those without a basis, and the projection step that keeps
//! expired results out of later model requests.
use super::revocation::Journal;
use crate::{
	Result,
	ports::research::{ProviderResultRetention, RetainedResult},
};
use aidash_domain::{
	context::{Context, ContextEvent},
	research::{
		account::{AccountProfile, Termination, basis_ends_at},
		expiry::{self, ExpiredResults},
	},
};
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Attempts examined per round trip.
pub const BATCH: u32 = 256;

/// Purge every Provider result whose basis ended at `now` under `current`,
/// the account terms the Deployment declares now, and the recorded
/// terminations. Terminations `current` names are recorded first, so they
/// outlive their declarations. Returns how many attempts were purged.
pub async fn sweep(
	port: &dyn ProviderResultRetention,
	current: &[AccountProfile],
	now: DateTime<Utc>,
	batch: u32,
) -> Result<u64> {
	let declared = current
		.iter()
		.filter_map(AccountProfile::termination)
		.collect::<Vec<_>>();
	if !declared.is_empty() {
		port.record_terminations(&declared).await?;
	}
	let terminations = port
		.terminations()
		.await?
		.into_iter()
		.map(|termination| (termination.account_id.clone(), termination))
		.collect::<BTreeMap<_, _>>();
	let current_days = current
		.iter()
		.map(|account| (account.account_id.as_str(), account.retention_days))
		.collect::<BTreeMap<_, _>>();
	// Admitted terms keep results for at least a day, so younger results are
	// not even examined, except those of an account whose deletion deadline
	// after termination has passed.
	let retrieved_before = now - Duration::days(1);
	let ended = terminations
		.values()
		.filter(|termination| deadline(termination) <= now)
		.map(|termination| termination.account_id.clone())
		.collect::<Vec<_>>();
	let expired = |candidate: &RetainedResult| {
		basis_ends_at(
			candidate.retrieved_at,
			candidate.retention_days,
			current_days.get(candidate.account_id.as_str()).copied(),
			terminations.get(&candidate.account_id),
		) <= now
	};
	let batch = batch.max(1);
	let mut after: Option<Uuid> = None;
	let mut purged = 0;
	loop {
		let candidates = port
			.candidates(after, retrieved_before, &ended, batch)
			.await?;
		let Some(last) = candidates.last() else {
			break;
		};
		after = Some(last.attempt_id);
		let ids = candidates
			.iter()
			.filter(|candidate| expired(candidate))
			.map(|candidate| candidate.attempt_id)
			.collect::<Vec<_>>();
		if !ids.is_empty() {
			purged += port.purge(&ids, now).await?;
		}
		if candidates.len() < batch as usize {
			break;
		}
	}
	Ok(purged)
}

fn deadline(termination: &Termination) -> DateTime<Utc> {
	termination.terminated_at
		+ Duration::days(i64::from(termination.deletion_after_termination_days))
}

/// A result compaction cut to a string: only the journaled original tells
/// which Provider result it carried.
fn cut(event: &ContextEvent) -> bool {
	matches!(event, ContextEvent::Tool { result, .. } if crate::context::truncated_result(result))
}

/// Keep a Run's expired Provider results out of its projection (ADR 0058).
/// Every research result built from one becomes the expired placeholder;
/// its call's arguments stay. Purges newer than the projection's watermark
/// are also looked up in the Context Journal: a result compaction cut to a
/// string is replaced when its original carried an expired result, and an
/// Execution Summary that absorbed one is discarded and its range restored
/// from the journal, so the Run continues without pausing. The Context
/// Journal itself is never edited. Returns whether the projection changed.
pub async fn expire_projection(
	context: &mut Context,
	expired: &ExpiredResults,
	journal: &dyn Journal,
) -> Result<bool> {
	if expired.is_empty() {
		return Ok(false);
	}
	let fresh = expired.through > context.research_expired_through;
	let mut changed = fresh;
	let mut originals = BTreeSet::new();
	if fresh {
		if let Some(summary) = context.execution_summary.as_deref() {
			let source = summary.source.clone();
			let entries = journal.entries(source.from_seq, source.through_seq).await?;
			if entries
				.iter()
				.any(|entry| source.absorbs(entry.seq) && expired.carried_by(&entry.event))
			{
				crate::context::restore_summarized(context, entries)?;
				crate::context::record_stage("summary", "research_expired");
			}
		}
		let cut = context
			.history
			.iter()
			.filter(|entry| cut(&entry.event))
			.map(|entry| entry.seq)
			.collect::<Vec<_>>();
		if let (Some(&from), Some(&through)) = (cut.first(), cut.last()) {
			originals = journal
				.entries(from, through)
				.await?
				.into_iter()
				.filter(|entry| cut.contains(&entry.seq) && expired.carried_by(&entry.event))
				.map(|entry| entry.seq)
				.collect();
		}
		context.research_expired_through = expired.through;
	}
	changed |= expiry::expire(&mut context.history, expired, &originals);
	Ok(changed)
}

#[cfg(test)]
mod tests;
