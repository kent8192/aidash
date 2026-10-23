//! PostgreSQL Research evidence for the host-owned research use cases.
use crate::apps::execution::models::research::{
	ResearchAttempt, ResearchFragment, ResearchOriginal, ResearchPageImage, ResearchRevocation,
	ResearchSnapshot, ResearchSource, lock, revoked_result,
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
	document::{ExtractedPage, Passage, RenderedPage, text_digest},
	evidence::{
		AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, FragmentImage, Limits,
		Original, RunUsage, SourceRecord,
	},
	search::SourceCandidate,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) mod account_terms;
pub(crate) mod retention;
mod revocation;
pub(crate) use revocation::{Revocations, scrub_revoked_projections};

/// A transaction for retention work, which reaches every Tenant's Research
/// evidence: purging snapshots and catching up revoked Runs.
async fn retention_transaction(
	store: &Store,
) -> crate::Result<Box<dyn reinhardt::db::backends::TransactionExecutor>> {
	use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
	store
		.pool
		.begin(&TenancyScope::Deployment(DeploymentScopeReason::Retention))
		.await?
		.into_executor()
		.await
}

#[derive(Clone)]
pub(crate) struct Evidence {
	pub store: Store,
	/// The Run's Tenant scope.
	pub scope: aidash_application::tenancy::TenancyScope,
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
					// Resolve the Run's Tenant before taking a policy lease. The
					// cross-Tenant listing scope is read-only and cannot hold it.
					let scope = self.federation.store.run_tenancy(run).await?;
					let inspected = self.federation.store.inspect_run(&scope, run).await?;
					crate::authorization::remote::deployment_administrator::require(
						&mut self.federation.store.pool.begin(&scope).await?,
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

/// Take the Run's research lock in `tx` and report whether its evidence was
/// revoked. A revocation holds the same lock while it closes the Run's Web
/// disclosures, so one admitted under this lock either commits before that
/// cleanup, which then closes it, or sees the revocation.
pub(crate) async fn revoked_under_lock(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: Uuid,
) -> crate::Result<bool> {
	lock(tx, &run_lock(run)).await?;
	ResearchRevocation::exists(tx, run).await
}

#[async_trait]
impl EvidenceStore for Evidence {
	async fn revoked(&self, run: Uuid) -> Result<bool> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
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
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
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
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				let search_attempts =
					ResearchAttempt::count(tx.as_mut(), run, Operation::Search).await?;
				let fetch_attempts =
					ResearchAttempt::count(tx.as_mut(), run, Operation::Fetch).await?;
				let sources = ResearchSource::count(tx.as_mut(), run).await?;
				let (snapshots, snapshot_bytes) = ResearchSnapshot::usage(tx.as_mut(), run).await?;
				let page_views = ResearchFragment::page_views(tx.as_mut(), run).await?;
				tx.commit().await?;
				Ok(RunUsage {
					search_attempts,
					fetch_attempts,
					sources,
					snapshots,
					snapshot_bytes,
					page_views,
				})
			}
			.await,
		)
	}
	async fn account_spend(&self, account: &str, since: DateTime<Utc>) -> Result<u64> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
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
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
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
	) -> Result<Value> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				// An attempt dispatched before its Run was revoked, or before
				// account terms expired it, records no evidence: it settles,
				// still charged, with a tombstone, which is also what its
				// caller returns and persists.
				lock(tx.as_mut(), &run_lock(run)).await?;
				let replaced = if ResearchRevocation::exists(tx.as_mut(), run).await? {
					ResearchAttempt::operation_of(tx.as_mut(), run, key)
						.await?
						.map(revoked_result)
				} else {
					ResearchAttempt::expired_tombstone(tx.as_mut(), run, key).await?
				};
				let result = replaced.as_ref().unwrap_or(result);
				if !ResearchAttempt::settle(tx.as_mut(), run, key, outcome, result).await? {
					return Err(crate::Error::Conflict(
						"research attempt is not dispatched".into(),
					));
				}
				tx.commit().await?;
				Ok(result.clone())
			}
			.await,
		)
	}
	async fn record_sources(
		&self,
		run: Uuid,
		key: &str,
		sources: &[SourceCandidate],
		max_sources: u32,
	) -> Result<std::result::Result<Vec<SourceRecord>, Failure>> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				lock(tx.as_mut(), &run_lock(run)).await?;
				if ResearchRevocation::exists(tx.as_mut(), run).await? {
					return Ok(Err(Failure::EvidenceRevoked));
				}
				let held = ResearchSource::count(tx.as_mut(), run).await?;
				let room = max_sources.saturating_sub(held) as usize;
				let mut records = Vec::with_capacity(sources.len().min(room));
				for source in sources.iter().take(room) {
					records.push(
						ResearchSource::record(tx.as_mut(), run, key, &source.url, &source.title)
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
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
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
		key: &str,
		source: Uuid,
		requested_url: &str,
		page: &ExtractedPage,
		limits: &Limits,
	) -> Result<std::result::Result<DocumentSnapshot, Failure>> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
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
				// An original is retained only within the Run's limit; beyond it
				// the snapshot records its digest alone (ADR 0053).
				let retained = match &page.original {
					Some(bytes) => {
						ResearchOriginal::held(tx.as_mut(), run)
							.await?
							.saturating_add(bytes.len() as u64)
							<= limits.original_bytes
					}
					None => false,
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
					page_count: page.page_count,
					pages_without_text: page.pages_without_text.clone(),
					original: page.original.as_ref().map(|bytes| Original {
						digest: format!("sha256:{:x}", Sha256::digest(bytes)),
						bytes: bytes.len() as u64,
						retained,
					}),
					purged: None,
				};
				ResearchSnapshot::record(tx.as_mut(), key, &snapshot).await?;
				if let Some(bytes) = page.original.as_deref().filter(|_| retained) {
					ResearchOriginal::record(tx.as_mut(), run, snapshot.id, bytes).await?;
				}
				tx.commit().await?;
				Ok(Ok(snapshot))
			}
			.await,
		)
	}
	async fn snapshot(&self, run: Uuid, id: Uuid) -> Result<Option<DocumentSnapshot>> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				let snapshot = ResearchSnapshot::get(tx.as_mut(), run, id).await?;
				tx.commit().await?;
				Ok(snapshot)
			}
			.await,
		)
	}
	async fn original(&self, run: Uuid, snapshot: Uuid) -> Result<Option<Vec<u8>>> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				let original = ResearchOriginal::get(tx.as_mut(), run, snapshot).await?;
				tx.commit().await?;
				Ok(original)
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
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				lock(tx.as_mut(), &run_lock(run)).await?;
				if ResearchRevocation::exists(tx.as_mut(), run).await? {
					return Ok(Err(Failure::EvidenceRevoked));
				}
				// The caller read the snapshot earlier; a retention purge that
				// committed since must not leave its contents in a fragment.
				if let Err(unreadable) =
					ResearchSnapshot::hold(tx.as_mut(), run, document.id).await?
				{
					return Ok(Err(unreadable));
				}
				ResearchFragment::record(tx.as_mut(), &fragment).await?;
				tx.commit().await?;
				Ok(Ok(fragment))
			}
			.await,
		)
	}
	async fn record_page_image(
		&self,
		run: Uuid,
		document: &DocumentSnapshot,
		page: u32,
		image: &RenderedPage,
		limits: &Limits,
	) -> Result<std::result::Result<EvidenceFragment, Failure>> {
		native(
			async {
				let fragment = EvidenceFragment::page_image(
					EvidenceRef(Uuid::new_v4()),
					document,
					FragmentImage {
						page,
						media_type: image.media_type.clone(),
						width: image.width,
						height: image.height,
						digest: image.digest(),
					},
				);
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				lock(tx.as_mut(), &run_lock(run)).await?;
				if ResearchRevocation::exists(tx.as_mut(), run).await? {
					return Ok(Err(Failure::EvidenceRevoked));
				}
				// A purge that committed since the caller read the snapshot took
				// the original the image was rendered from.
				if let Err(unreadable) =
					ResearchSnapshot::hold(tx.as_mut(), run, document.id).await?
				{
					return Ok(Err(unreadable));
				}
				// A concurrent view of the same page recorded it first.
				if let Some(viewed) =
					ResearchFragment::viewed(tx.as_mut(), run, document.id, page).await?
				{
					tx.commit().await?;
					return Ok(Ok(viewed));
				}
				if ResearchFragment::page_views(tx.as_mut(), run).await? >= limits.page_views {
					return Ok(Err(Failure::ViewLimit));
				}
				ResearchFragment::record(tx.as_mut(), &fragment).await?;
				ResearchPageImage::record(tx.as_mut(), run, fragment.reference.0, &image.bytes)
					.await?;
				tx.commit().await?;
				Ok(Ok(fragment))
			}
			.await,
		)
	}
	async fn viewed_page(
		&self,
		run: Uuid,
		document: Uuid,
		page: u32,
	) -> Result<Option<EvidenceFragment>> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				let viewed = if ResearchRevocation::exists(tx.as_mut(), run).await? {
					None
				} else {
					ResearchFragment::viewed(tx.as_mut(), run, document, page).await?
				};
				tx.commit().await?;
				Ok(viewed)
			}
			.await,
		)
	}
	async fn page_image(
		&self,
		run: Uuid,
		reference: &EvidenceRef,
	) -> Result<Option<(String, Vec<u8>)>> {
		native(
			async {
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				let image = if ResearchRevocation::exists(tx.as_mut(), run).await? {
					None
				} else {
					let media_type = ResearchFragment::delivered(
						tx.as_mut(),
						run,
						std::slice::from_ref(reference),
					)
					.await?
					.pop()
					.and_then(|fragment| fragment.image)
					.map(|image| image.media_type);
					match media_type {
						Some(media_type) => ResearchPageImage::get(tx.as_mut(), run, reference.0)
							.await?
							.map(|bytes| (media_type, bytes)),
						None => None,
					}
				};
				tx.commit().await?;
				Ok(image)
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
				let mut tx = self
					.store
					.pool
					.begin(&self.scope)
					.await?
					.into_executor()
					.await?;
				// Revoked evidence is never delivered: later citations of it are
				// unknown, and readers get the uniform absence (ADR 0046). The
				// Run's research lock, which a revocation holds, keeps the check
				// and the read on one side of it.
				lock(tx.as_mut(), &run_lock(run)).await?;
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

/// One Run's research usage, charges and unsettled attempts. The caller has
/// already decided that the reader may read the Run, and fills in the
/// reader's standing towards the Run's Evidence Revocation.
pub(crate) async fn run_research(
	store: &Store,
	scope: &aidash_application::tenancy::TenancyScope,
	run: Uuid,
	limits: Limits,
) -> crate::Result<crate::apps::execution::serializers::runs::RunResearch> {
	let usage = Evidence {
		store: store.clone(),
		scope: scope.clone(),
	}
	.usage(run)
	.await?;
	let mut tx = store.pool.begin(scope).await?.into_executor().await?;
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
		revoked_at: None,
		can_revoke: false,
		can_read_revocation: false,
	})
}
