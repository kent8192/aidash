//! PostgreSQL Research evidence for the host-owned research use cases.
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchSnapshot, ResearchSource,
};
use crate::store::Store;
use aidash_application::{
	Result,
	ports::research::{EvidenceStore, Reservation, Reserved},
};
use aidash_domain::research::{
	Operation,
	citations::EvidenceRef,
	document::{ExtractedPage, Passage, text_digest},
	evidence::{
		AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, RunUsage, SourceRecord,
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
				ResearchAttempt::settle(tx.as_mut(), run, key, outcome, result).await?;
				tx.commit().await
			}
			.await,
		)
	}
	async fn record_sources(
		&self,
		run: Uuid,
		_: &str,
		sources: &[SourceCandidate],
	) -> Result<Vec<SourceRecord>> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let mut records = Vec::with_capacity(sources.len());
				for source in sources {
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
	) -> Result<DocumentSnapshot> {
		native(
			async {
				let mut tx = self.store.database().begin().await?;
				let source_id = match source {
					Some(id) => {
						ResearchSource::get(tx.as_mut(), run, id)
							.await?
							.ok_or_else(|| crate::Error::NotFound("source unavailable".into()))?
							.id
					}
					None => {
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
				Ok(snapshot)
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
