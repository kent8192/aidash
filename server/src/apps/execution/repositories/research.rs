//! PostgreSQL Research evidence for the host-owned research use cases.
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchSnapshot, ResearchSource, lock,
};
use crate::store::Store;
use aidash_application::{
	Result,
	ports::research::{EvidenceStore, Reservation, Reserved},
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

#[derive(Clone)]
pub(crate) struct Evidence {
	pub store: Store,
}

fn native<T>(result: crate::Result<T>) -> Result<T> {
	result.map_err(Into::into)
}

#[async_trait]
impl EvidenceStore for Evidence {
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
	) -> Result<Vec<SourceRecord>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				lock(tx.as_mut(), &format!("research-run:{run}")).await?;
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
				Ok(records)
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
		source: Option<Uuid>,
		requested_url: &str,
		page: &ExtractedPage,
		limits: &Limits,
	) -> Result<std::result::Result<DocumentSnapshot, Failure>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				lock(tx.as_mut(), &format!("research-run:{run}")).await?;
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
				let source_id = match source {
					Some(id) => {
						ResearchSource::get(tx.as_mut(), run, id)
							.await?
							.ok_or_else(|| crate::Error::NotFound("source unavailable".into()))?
							.id
					}
					None => {
						if ResearchSource::count(tx.as_mut(), run).await? >= limits.sources {
							return Ok(Err(Failure::SourceLimit));
						}
						ResearchSource::record(
							tx.as_mut(),
							run,
							&page.url,
							page.title.as_deref().unwrap_or_default(),
						)
						.await?
						.id
					}
				};
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
		_: Uuid,
		document: &DocumentSnapshot,
		passage: &Passage,
	) -> Result<EvidenceFragment> {
		native(
			async {
				let fragment =
					EvidenceFragment::new(EvidenceRef(Uuid::new_v4()), document, passage);
				let mut tx = self.store.database().begin().await?;
				ResearchFragment::record(tx.as_mut(), &fragment).await?;
				tx.commit().await?;
				Ok(fragment)
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
				let fragments = ResearchFragment::delivered(tx.as_mut(), run, references).await?;
				tx.commit().await?;
				Ok(fragments)
			}
			.await,
		)
	}
}
