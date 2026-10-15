//! PostgreSQL Research evidence for the host-owned research use cases.
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchSnapshot, ResearchSource, lock,
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

#[derive(Clone)]
pub(crate) struct Evidence {
	pub store: Store,
}

fn native<T>(result: crate::Result<T>) -> Result<T> {
	result.map_err(Into::into)
}

/// Run readability for one API reader, decided exactly as `GET /api/runs/{id}`
/// decides it: a Subject through Run inspection's visibility decision, an
/// Operator through the Workspace's operator visibility.
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
			Actor::Operator => native(
				async {
					let inspected = self.federation.store.inspect_run(run).await?;
					crate::authorization::remote::operator::require(
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
		source: Uuid,
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

/// One Run's research usage, charges and unsettled attempts. The caller has
/// already decided that the reader may read the Run.
pub(crate) async fn run_research(
	store: &Store,
	run: Uuid,
	limits: Limits,
) -> crate::Result<crate::apps::execution::serializers::runs::RunResearch> {
	let usage = Evidence {
		store: store.clone(),
	}
	.usage(run)
	.await?;
	let mut tx = store.database().begin().await?;
	let (estimated_micro_usd, uncertain_attempts, in_flight_attempts) =
		ResearchAttempt::run_charges(tx.as_mut(), run).await?;
	let url_intents =
		crate::apps::execution::models::url_intents::UrlIntentRecord::for_run(tx.as_mut(), run)
			.await?;
	tx.commit().await?;
	use crate::apps::execution::serializers::runs::{RunUrlIntent, UrlIntentInput};
	Ok(crate::apps::execution::serializers::runs::RunResearch {
		limits: limits.into(),
		usage: usage.into(),
		estimated_micro_usd,
		uncertain_attempts,
		in_flight_attempts,
		url_intents: url_intents
			.into_iter()
			.map(|record| RunUrlIntent {
				input: if record.input_key.is_some() {
					UrlIntentInput::RunMessage
				} else {
					UrlIntentInput::Task
				},
				pages: record.pages.0,
				requested_by: record.requested_by,
				requested_at: record.created_at,
			})
			.collect(),
	})
}
