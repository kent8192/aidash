//! Evidence Revocation of all of one Run's Research evidence (ADR 0046).
use crate::{Error, Result, ports::research::RevocationRepository};
use aidash_domain::{
	context::{Context, ContextEvent, HistoryEntry},
	research::revocation::{self, EvidenceRevocation, REVOKE, Removed},
	retention::{self, Decider},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Revoke all of one Run's Research evidence. Revocation is irreversible:
/// revoking a Run again returns its first record unchanged.
pub async fn revoke(
	repository: &dyn RevocationRepository,
	run: Uuid,
	reason: &str,
	now: DateTime<Utc>,
) -> Result<EvidenceRevocation> {
	let reason = revocation::reason(reason)?;
	let mut scope = repository.begin().await?;
	let result = async {
		let decider = scope.decider();
		match decider {
			Decider::Subject { .. } => scope.require(run, REVOKE).await?,
			// A Deployment Administrator reads no content to revoke; only the
			// Run's existence is checked.
			Decider::DeploymentAdministrator { .. } => {
				if !scope.run_exists(run).await? {
					return Err(Error::NotFound("run unavailable".into()));
				}
			}
		}
		let tenant = scope.tenant(run).await?;
		scope
			.revoke(&EvidenceRevocation {
				run_id: run,
				tenant,
				revoked_by: decider,
				reason,
				revoked_at: now,
				removed: Removed::default(),
			})
			.await
	}
	.await;
	let revoked = scope.finish(result).await?;
	repository.notify();
	Ok(revoked)
}

/// The revocation records of `tenant`, newest first, for a subject of that
/// Tenant permitted `research.revoke` or a Deployment Administrator.
pub async fn records(
	repository: &dyn RevocationRepository,
	tenant: &str,
	offset: u64,
	limit: u32,
) -> Result<Vec<EvidenceRevocation>> {
	let mut scope = repository.begin_records().await?;
	let result = async {
		if let Decider::Subject { tenant: own, .. } = scope.decider()
			&& own != tenant
		{
			return Err(Error::Forbidden);
		}
		scope.require(tenant, REVOKE).await?;
		scope
			.records(tenant, offset, limit.clamp(1, retention::MAX_PAGE))
			.await
	}
	.await;
	scope.finish(result.is_ok()).await?;
	result
}

/// A Run's Context Journal, which keeps every original event.
#[async_trait]
pub trait Journal: Send + Sync {
	/// The original entries with `from <= seq <= through`, in order.
	async fn entries(&self, from: u64, through: u64) -> Result<Vec<HistoryEntry>>;
}

/// One Run's Context Journal through its execution store.
pub struct RunJournal<'a> {
	pub store: &'a dyn crate::ports::execution::ExecutionStore,
	pub run: Uuid,
}

#[async_trait]
impl Journal for RunJournal<'_> {
	async fn entries(&self, from: u64, through: u64) -> Result<Vec<HistoryEntry>> {
		self.store.context_journal(self.run, from, through).await
	}
}

/// A result compaction cut to a string or elided whole, keeping its call's
/// arguments: only the journaled original tells whether it was research.
fn cut(event: &ContextEvent) -> bool {
	matches!(event, ContextEvent::Tool { result, .. }
		if crate::context::truncated_result(result) || ContextEvent::is_elided_result(result))
}

/// Whether a projection not yet revoked may hold research content: a research
/// result, a result compaction cut to a string or elided, or an Execution
/// Summary.
pub fn may_carry_research(context: &Context) -> bool {
	context.execution_summary.is_some()
		|| context
			.history
			.iter()
			.any(|entry| revocation::carries_research(&entry.event) || cut(&entry.event))
}

/// Apply a Run's Evidence Revocation to its projection (ADR 0046), so no
/// later model request and no reader of the projection sees the revoked
/// content. Every research result becomes a placeholder, including one that
/// compaction cut to a string whose journaled original was a research result.
/// An Execution Summary that absorbed any research result is discarded: its
/// range is restored from the journal and redacted the same way. The first
/// pass marks the projection, so later passes only redact appended results.
/// The Context Journal itself is never edited.
pub async fn revoke_projection(context: &mut Context, journal: &dyn Journal) -> Result<()> {
	let mut originals = BTreeSet::new();
	if !context.research_revoked {
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
				.filter(|entry| {
					cut.contains(&entry.seq) && revocation::carries_research(&entry.event)
				})
				.map(|entry| entry.seq)
				.collect();
		}
		if let Some(summary) = context.execution_summary.as_deref() {
			let source = summary.source.clone();
			let entries = journal.entries(source.from_seq, source.through_seq).await?;
			if entries.iter().any(|entry| {
				source.absorbs(entry.seq) && revocation::carries_research(&entry.event)
			}) {
				crate::context::restore_summarized(context, entries)?;
				crate::context::record_stage("summary", "research_revoked");
			}
		}
		context.research_revoked = true;
	}
	revocation::redact(&mut context.history, &originals);
	Ok(())
}

#[cfg(test)]
mod tests;
