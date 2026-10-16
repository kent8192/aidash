//! PostgreSQL Research evidence for the host-owned research use cases.
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchRevocation, ResearchSnapshot, ResearchSource, lock,
	revoked_result,
};
use crate::store::Store;
use crate::{authorization::identity::Actor, federation::Federation};
use aidash_application::{
	Error, Result,
	ports::research::{EvidenceStore, Reservation, Reserved, RunReader},
};
use aidash_domain::research::{
	Failure, Operation,
	citations::EvidenceRef,
	document::{ExtractedPage, Passage, text_digest},
	evidence::{
		AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, Limits, RunUsage,
		SourceRecord,
	},
	search::SourceCandidate,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

pub(crate) mod retention;
mod revocation;
pub(crate) use revocation::{Revocations, scrub_revoked_projections};

#[derive(Clone)]
pub(crate) struct Evidence {
	pub store: Store,
}

fn native<T>(result: crate::Result<T>) -> Result<T> {
	result.map_err(Into::into)
}

/// Run readability for one API reader, decided exactly as `GET /api/runs/{id}`
/// decides it: a Subject through Run inspection's visibility decision, an
/// Deployment Administrator through the Workspace's Deployment Administrator visibility.
pub(crate) struct Readers<'a> {
	pub federation: &'a Federation,
	pub actor: &'a Actor,
}

#[async_trait]
impl RunReader for Readers<'_> {
	async fn readable(&self, run: Uuid) -> Result<bool> {
		let decision = match self.actor {
			Actor::Subject(identity) => aidash_application::authorization::run_details::visible(
				&crate::bootstrap::run_details_repository(self.federation, identity),
				run,
			)
			.await
			.map(|_| ()),
			Actor::DeploymentAdministrator => native(
				async {
					let inspected = self.federation.store.inspect_run(run).await?;
					crate::authorization::remote::deployment_administrator::require(
						&mut crate::database::native::begin(&self.federation.store.pool).await?,
						inspected.workspace_id,
					)
					.await
				}
				.await,
			),
		};
		match decision {
			Ok(()) => Ok(true),
			Err(Error::Forbidden | Error::NotFound(_)) => Ok(false),
			Err(error) => Err(error),
		}
	}
}

/// The advisory lock that serializes one Run's research writes with its
/// Evidence Revocation.
fn run_lock(run: Uuid) -> String {
	format!("research-run:{run}")
}

#[async_trait]
impl EvidenceStore for Evidence {
	async fn revoked(&self, run: Uuid) -> Result<bool> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let revoked = ResearchRevocation::exists(tx.as_mut(), run).await?;
				tx.commit().await?;
				Ok(revoked)
			}
			.await,
		)
	}
	async fn attempt(&self, run: Uuid, key: &str) -> Result<Option<AttemptState>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let state = ResearchAttempt::find(tx.as_mut(), run, key).await?;
				tx.commit().await?;
				Ok(state)
			}
			.await,
		)
	}
	async fn usage(&self, run: Uuid) -> Result<RunUsage> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let search_attempts =
					ResearchAttempt::count(tx.as_mut(), run, Operation::Search).await?;
				let fetch_attempts =
					ResearchAttempt::count(tx.as_mut(), run, Operation::Fetch).await?;
				let sources = ResearchSource::count(tx.as_mut(), run).await?;
				let (snapshots, snapshot_bytes) = ResearchSnapshot::usage(tx.as_mut(), run).await?;
				tx.commit().await?;
				Ok(RunUsage {
					search_attempts,
					fetch_attempts,
					sources,
					snapshots,
					snapshot_bytes,
				})
			}
			.await,
		)
	}
	async fn account_spend(&self, account: &str, since: DateTime<Utc>) -> Result<u64> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let spent = ResearchAttempt::spend(tx.as_mut(), account, since).await?;
				tx.commit().await?;
				Ok(spent)
			}
			.await,
		)
	}
	async fn reserve(&self, run: Uuid, reservation: &Reservation) -> Result<Reserved> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let reserved = ResearchAttempt::reserve(tx.as_mut(), run, reservation).await?;
				tx.commit().await?;
				Ok(reserved)
			}
			.await,
		)
	}
	async fn settle(
		&self,
		run: Uuid,
		key: &str,
		outcome: AttemptOutcome,
		result: &Value,
	) -> Result<()> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				// An attempt dispatched before its Run was revoked records no
				// evidence: it settles, still charged, with a tombstone.
				lock(tx.as_mut(), &run_lock(run)).await?;
				let revoked = if ResearchRevocation::exists(tx.as_mut(), run).await? {
					ResearchAttempt::operation_of(tx.as_mut(), run, key)
						.await?
						.map(revoked_result)
				} else {
					None
				};
				let result = revoked.as_ref().unwrap_or(result);
				if !ResearchAttempt::settle(tx.as_mut(), run, key, outcome, result).await? {
					return Err(crate::Error::Conflict(
						"research attempt is not dispatched".into(),
					));
				}
				tx.commit().await?;
				Ok(())
			}
			.await,
		)
	}
	async fn record_sources(
		&self,
		run: Uuid,
		_: &str,
		sources: &[SourceCandidate],
		max_sources: u32,
	) -> Result<std::result::Result<Vec<SourceRecord>, Failure>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				lock(tx.as_mut(), &run_lock(run)).await?;
				if ResearchRevocation::exists(tx.as_mut(), run).await? {
					return Ok(Err(Failure::EvidenceRevoked));
				}
				let held = ResearchSource::count(tx.as_mut(), run).await?;
				let room = max_sources.saturating_sub(held) as usize;
				let mut records = Vec::with_capacity(sources.len().min(room));
				for source in sources.iter().take(room) {
					records.push(
						ResearchSource::record(tx.as_mut(), run, &source.url, &source.title)
							.await?,
					);
				}
				tx.commit().await?;
				Ok(Ok(records))
			}
			.await,
		)
	}
	async fn source(&self, run: Uuid, id: Uuid) -> Result<Option<SourceRecord>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let source = ResearchSource::get(tx.as_mut(), run, id).await?;
				tx.commit().await?;
				Ok(source)
			}
			.await,
		)
	}
	async fn record_snapshot(
		&self,
		run: Uuid,
		source: Uuid,
		requested_url: &str,
		page: &ExtractedPage,
		limits: &Limits,
	) -> Result<std::result::Result<DocumentSnapshot, Failure>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				lock(tx.as_mut(), &run_lock(run)).await?;
				if ResearchRevocation::exists(tx.as_mut(), run).await? {
					return Ok(Err(Failure::EvidenceRevoked));
				}
				let (snapshots, held) = ResearchSnapshot::usage(tx.as_mut(), run).await?;
				let bytes = page
					.lines
					.iter()
					.map(|line| line.text.len() as u64)
					.sum::<u64>();
				if snapshots >= limits.snapshots
					|| held.saturating_add(bytes) > limits.snapshot_bytes
				{
					return Ok(Err(Failure::SnapshotLimit));
				}
				let source_id = ResearchSource::get(tx.as_mut(), run, source)
					.await?
					.ok_or_else(|| crate::Error::NotFound("source unavailable".into()))?
					.id;
				let snapshot = DocumentSnapshot {
					id: Uuid::now_v7(),
					run_id: run,
					source_id,
					requested_url: requested_url.into(),
					url: page.url.clone(),
					title: page.title.clone(),
					media_type: page.media_type.clone(),
					fetched_at: page.fetched_at,
					raw_digest: page.raw_digest.clone(),
					text_digest: text_digest(&page.lines),
					completeness: page.completeness,
					lines: page.lines.clone(),
					purged: None,
				};
				ResearchSnapshot::record(tx.as_mut(), &snapshot).await?;
				tx.commit().await?;
				Ok(Ok(snapshot))
			}
			.await,
		)
	}
	async fn snapshot(&self, run: Uuid, id: Uuid) -> Result<Option<DocumentSnapshot>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let snapshot = ResearchSnapshot::get(tx.as_mut(), run, id).await?;
				tx.commit().await?;
				Ok(snapshot)
			}
			.await,
		)
	}
	async fn record_fragment(
		&self,
		run: Uuid,
		document: &DocumentSnapshot,
		passage: &Passage,
	) -> Result<std::result::Result<EvidenceFragment, Failure>> {
		native(
			async {
				let fragment =
					EvidenceFragment::new(EvidenceRef(Uuid::new_v4()), document, passage);
				let mut tx = self.store.database().begin().await?;
				lock(tx.as_mut(), &run_lock(run)).await?;
				if ResearchRevocation::exists(tx.as_mut(), run).await? {
					return Ok(Err(Failure::EvidenceRevoked));
				}
				ResearchFragment::record(tx.as_mut(), &fragment).await?;
				tx.commit().await?;
				Ok(Ok(fragment))
			}
			.await,
		)
	}
	async fn fragments(
		&self,
		run: Uuid,
		references: &[EvidenceRef],
	) -> Result<Vec<EvidenceFragment>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				// Revoked evidence is never delivered: later citations of it are
				// unknown, and readers get the uniform absence (ADR 0046).
				let fragments = if ResearchRevocation::exists(tx.as_mut(), run).await? {
					Vec::new()
				} else {
					ResearchFragment::delivered(tx.as_mut(), run, references).await?
				};
				tx.commit().await?;
				Ok(fragments)
			}
			.await,
		)
	}
}
