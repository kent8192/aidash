//! Ports of the host-owned research contract. Provider packages implement
//! [`SearchProvider`] or [`PageProvider`]; the Node implements the evidence
//! store and the Web disclosure gate once for every provider.
use crate::Result;
use aidash_domain::research::{
	Failure, Operation,
	account::{AccountTerms, CostAttribution, Termination},
	citations::EvidenceRef,
	document::{ExtractedPage, Passage, RenderedPage},
	evidence::{
		AttemptOutcome, AttemptState, DocumentSnapshot, EvidenceFragment, Limits, RunUsage,
		SourceRecord,
	},
	revocation::EvidenceRevocation,
	search::{ProviderResults, SearchQuery, SourceCandidate},
};
use aidash_domain::retention::{Decider, RetentionRules, SnapshotPeriods};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use url::Url;
use uuid::Uuid;

/// The exact content a provider sends outside the Node for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
	/// Host that receives the content.
	pub destination: String,
	/// The exact query or page address that leaves the Node.
	pub content: String,
}

/// A Web search provider. It translates the validated query without relaxing
/// its filters; the host filters, bounds and records every candidate.
#[async_trait]
pub trait SearchProvider: Send + Sync {
	/// The exact outbound content; a query the provider cannot send unchanged
	/// is rejected here, before any approval is requested.
	fn disclosure(&self, query: &SearchQuery) -> std::result::Result<Outbound, Failure>;
	/// The Research account holder's current account terms, or `None` for a
	/// provider that charges nothing.
	fn account(&self) -> std::result::Result<Option<AccountTerms>, Failure>;
	async fn search(&self, query: &SearchQuery) -> std::result::Result<ProviderResults, Failure>;
}

/// A Page reading provider. It must refuse non-public destinations itself,
/// including addresses a public name resolves to at dispatch.
#[async_trait]
pub trait PageProvider: Send + Sync {
	/// Only Aidash's own reader may bypass external account terms.
	fn deployment_hosted(&self) -> bool {
		false
	}

	/// Literal address checks performed before any disclosure or attempt.
	fn admit(&self, url: &Url) -> std::result::Result<(), Failure>;
	/// The Research account holder's current account terms, or `None` for a
	/// provider that charges nothing, such as Aidash's own page reader, whose
	/// pages are not Provider results (ADR 0058).
	fn account(&self) -> std::result::Result<Option<AccountTerms>, Failure> {
		Ok(None)
	}
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure>;
}

/// Renders one page of a retained original as an image. It must keep within
/// the page image bounds; the host checks them again.
#[async_trait]
pub trait PageRenderer: Send + Sync {
	async fn render(&self, pdf: Vec<u8>, page: u32) -> std::result::Result<RenderedPage, Failure>;
}

/// A reservation made before a provider is called. A repeated invocation key
/// finds it instead of dispatching again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
	pub key: String,
	pub operation: Operation,
	pub provider: String,
	pub destination: String,
	pub attribution: Option<CostAttribution>,
	/// The account terms the result will be retrieved under, for a provider
	/// with Research account terms; recorded with the attempt (ADR 0058).
	pub retention: Option<RetrievalTerms>,
	/// The Run's research limits. Attempts of this operation, and the Run's
	/// Source records (search, or a page read by address) or Document
	/// snapshots (fetch, counting every still dispatched read as one), are
	/// checked under the same lock as the reservation.
	pub limits: Limits,
	/// The attributed account's charge limit; checked with the reservation.
	pub budget: Option<Budget>,
	/// The address of a page read by address rather than a Source record. Its
	/// Source record is created with the reservation, within the Run's Source
	/// record limit, so the slot is held before anything is sent.
	pub source_url: Option<String>,
}

/// The Research account terms a Provider result is retrieved under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalTerms {
	pub account_id: String,
	pub retention_days: u16,
}

impl RetrievalTerms {
	pub fn of(account: &AccountTerms) -> Self {
		Self {
			account_id: account.account_id.clone(),
			retention_days: account.retention_days,
		}
	}
}

/// Charges an account may hold since `since`, including the new attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budget {
	pub since: DateTime<Utc>,
	pub limit_micro_usd: u64,
}

/// The outcome of a reservation. Limits are checked atomically with it, so
/// concurrent invocations cannot together exceed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reserved {
	/// `source` is the Source record created for [`Reservation::source_url`].
	Reserved {
		source: Option<Uuid>,
	},
	/// The key already has an attempt.
	Duplicate,
	AttemptLimit,
	SourceLimit,
	SnapshotLimit,
	BudgetExhausted,
	/// The Run's evidence was revoked; checked under the same lock as the
	/// revocation, so nothing is dispatched after it commits.
	Revoked,
	/// This account ID was terminated and cannot be reused for a new agreement.
	AccountUnavailable,
}

/// Run-scoped Research evidence. Every read is limited to the given Run; an
/// identifier of another Run is reported as absent.
///
/// After an Evidence Revocation of the Run (ADR 0046) every write that would
/// record evidence is refused with [`Failure::EvidenceRevoked`] under the
/// Run's lock, settling stores a tombstone instead of the given result, and
/// no fragment is delivered any more.
#[async_trait]
pub trait EvidenceStore: Send + Sync {
	/// Current policy, reloaded before every external Research send.
	async fn data_protection(
		&self,
		_run: Uuid,
	) -> Result<aidash_domain::data_protection::DataProtection> {
		Ok(Default::default())
	}

	/// Whether the Run's Research evidence was revoked.
	async fn revoked(&self, run: Uuid) -> Result<bool>;
	/// Account expiry is independent of a snapshot's first ordinary purge reason.
	async fn expired_results(
		&self,
		_run: Uuid,
	) -> Result<aidash_domain::research::expiry::ExpiredResults> {
		Ok(Default::default())
	}
	async fn attempt(&self, run: Uuid, key: &str) -> Result<Option<AttemptState>>;
	/// Purged snapshots count toward neither the snapshot nor the byte limit,
	/// so a Run can read a Source again after its contents were purged.
	async fn usage(&self, run: Uuid) -> Result<RunUsage>;
	/// Charges attributed to `account` since `since`, including dispatched
	/// attempts whose outcome is still unknown.
	async fn account_spend(&self, account: &str, since: DateTime<Utc>) -> Result<u64>;
	async fn reserve(&self, run: Uuid, reservation: &Reservation) -> Result<Reserved>;
	/// Settles a dispatched attempt exactly once; settling a missing or
	/// already settled attempt is a `Conflict`. Returns the result it
	/// recorded, which is the revoked tombstone, not `result`, when the Run's
	/// evidence was revoked meanwhile; the caller returns that one.
	async fn settle(
		&self,
		run: Uuid,
		key: &str,
		outcome: AttemptOutcome,
		result: &Value,
	) -> Result<Value>;
	/// Source records in candidate order, recorded as created by the attempt
	/// of `key`.
	async fn record_sources(
		&self,
		run: Uuid,
		key: &str,
		sources: &[SourceCandidate],
		max_sources: u32,
	) -> Result<std::result::Result<Vec<SourceRecord>, Failure>>;
	async fn source(&self, run: Uuid, id: Uuid) -> Result<Option<SourceRecord>>;
	/// A new Document snapshot of the Source record `source`. Under the Run's
	/// lock, the snapshot is refused with `SnapshotLimit` when it would exceed
	/// the Run's snapshot limits, or with `EvidenceRevoked`.
	/// `key` is the invocation key of the attempt that read it.
	async fn record_snapshot(
		&self,
		run: Uuid,
		key: &str,
		source: Uuid,
		requested_url: &str,
		page: &ExtractedPage,
		limits: &Limits,
	) -> Result<std::result::Result<DocumentSnapshot, Failure>>;
	/// A Purged snapshot is returned with its purge and without contents.
	async fn snapshot(&self, run: Uuid, id: Uuid) -> Result<Option<DocumentSnapshot>>;
	/// The retained original of snapshot `snapshot` of `run`, if it is still
	/// kept (ADR 0053).
	async fn original(&self, run: Uuid, snapshot: Uuid) -> Result<Option<Vec<u8>>>;
	async fn record_fragment(
		&self,
		run: Uuid,
		document: &DocumentSnapshot,
		passage: &Passage,
	) -> Result<std::result::Result<EvidenceFragment, Failure>>;
	/// Record `image`, rendered from page `page` of `document`, as a page-image
	/// Evidence fragment. Under the Run's lock the fragment is refused when the
	/// evidence was revoked, the snapshot was purged, or the Run reached
	/// `limits.page_views` (ADR 0053).
	async fn record_page_image(
		&self,
		run: Uuid,
		document: &DocumentSnapshot,
		page: u32,
		image: &RenderedPage,
		limits: &Limits,
	) -> Result<std::result::Result<EvidenceFragment, Failure>>;
	/// The page-image fragment already delivered to this Run for `page` of
	/// `document`; none once the Run's evidence is revoked.
	async fn viewed_page(
		&self,
		run: Uuid,
		document: Uuid,
		page: u32,
	) -> Result<Option<EvidenceFragment>>;
	/// The media type and bytes of the page image `reference` delivered to this
	/// Run; absent once its evidence is revoked.
	async fn page_image(
		&self,
		run: Uuid,
		reference: &EvidenceRef,
	) -> Result<Option<(String, Vec<u8>)>>;
	/// The subset of `references` delivered to this Run; none once the Run's
	/// evidence is revoked, so later citations of it are unknown.
	async fn fragments(
		&self,
		run: Uuid,
		references: &[EvidenceRef],
	) -> Result<Vec<EvidenceFragment>>;
}

/// Whether the current reader may read a Run now, decided exactly as Run
/// inspection decides it for that reader. A Run that does not exist is not
/// readable.
#[async_trait]
pub trait RunReader: Send + Sync {
	async fn readable(&self, run: Uuid) -> Result<bool>;
}

/// One Web disclosure: Run-derived content leaving the Node through a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDisclosure {
	pub operation: Operation,
	pub provider: String,
	pub outbound: Outbound,
	/// The Run's observed input sequence; new user input needs a new approval.
	pub context_revision: i64,
	/// The Research account holder's account and price for this request, as
	/// they stand when approval is requested. Part of the digest, so an
	/// approval covers only the charge its approver was shown.
	pub attribution: Option<CostAttribution>,
	/// The host-validated search query in provider-independent form, so an
	/// approver reads it without parsing a provider's request body. Not part
	/// of the digest: the exact outbound content already binds it.
	pub query: Option<Value>,
}

impl WebDisclosure {
	/// Approval is bound to this exact digest.
	pub fn digest(&self, run: Uuid) -> String {
		aidash_domain::registry::rules::digest(&serde_json::json!([
			"web_disclosure",
			run,
			self.operation,
			self.provider,
			self.outbound.destination,
			self.outbound.content,
			self.context_revision,
			self.attribution,
		]))
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisclosureState {
	/// Policy or a Run grant permits the disclosure.
	Permitted,
	/// A person approved this exact disclosure once. The approval is consumed
	/// only after the attempt is reserved, by [`DisclosureGate::consume`].
	Approved {
		approval_id: Uuid,
	},
	/// An eligible person must decide first. The Run waits on this approval.
	Pending {
		approval_id: Uuid,
		expires_at: Option<DateTime<Utc>>,
	},
	Denied,
	/// The Run's evidence was revoked before the disclosure was admitted, so
	/// no approval is created to carry its content (ADR 0046).
	Revoked,
}

/// The Node's Web disclosure approval, built on its capability approvals.
#[async_trait]
pub trait DisclosureGate: Send + Sync {
	async fn admit(&self, run: Uuid, disclosure: &WebDisclosure) -> Result<DisclosureState>;
	/// Consume a one-time approval. `false` when it was already consumed or is
	/// no longer approved; the disclosure must then not be sent.
	async fn consume(&self, run: Uuid, approval_id: Uuid) -> Result<bool>;
}

/// One Evidence Revocation in one transaction (ADR 0046).
#[async_trait]
pub trait RevocationScope: Send {
	/// Who is revoking.
	fn decider(&self) -> Decider;
	/// Whether the Run exists. A subject is told `Forbidden` instead of
	/// `NotFound` by [`RevocationScope::require`], so it learns nothing.
	async fn run_exists(&mut self, run: Uuid) -> Result<bool>;
	/// A subject must read the Run's workspace, see the Run, and be permitted
	/// `action` on the Run's `research` resource.
	async fn require(&mut self, run: Uuid, action: &str) -> Result<()>;
	/// The Run's Tenant, when it has one.
	async fn tenant(&mut self, run: Uuid) -> Result<Option<String>>;
	/// Under the Run's research lock: replace every stored copy of the Run's
	/// source content with a tombstone, close its pending Web disclosure
	/// approvals and record `revocation` with what was removed. A Run already
	/// revoked keeps its first record, which is returned unchanged.
	async fn revoke(&mut self, revocation: &EvidenceRevocation) -> Result<EvidenceRevocation>;
	async fn finish(
		self: Box<Self>,
		result: Result<EvidenceRevocation>,
	) -> Result<EvidenceRevocation>;
}

/// Reading one Tenant's revocation records, which are part of its audit
/// history (ADR 0046).
#[async_trait]
pub trait RevocationRecordScope: Send {
	/// Who is reading.
	fn decider(&self) -> Decider;
	/// Whether a subject is permitted `action` on its Tenant's revocation
	/// records; a Deployment Administrator needs no Tenant policy.
	async fn permits(&mut self, tenant: &str, action: &str) -> Result<bool>;
	/// Whether a subject sees the Run and is permitted `action` on the Run's
	/// `research` resource, as [`RevocationScope::require`] decides it; a
	/// Deployment Administrator needs no Tenant policy.
	async fn permits_run(&mut self, run: Uuid, action: &str) -> Result<bool>;
	/// The Run's revocation record, if its evidence was revoked.
	async fn record(&mut self, run: Uuid) -> Result<Option<EvidenceRevocation>>;
	/// The Tenant's records, newest first.
	async fn records(
		&mut self,
		tenant: &str,
		offset: u64,
		limit: u32,
	) -> Result<Vec<EvidenceRevocation>>;
	async fn finish(self: Box<Self>, committed: bool) -> Result<()>;
}

#[async_trait]
pub trait RevocationRepository: Send + Sync {
	async fn begin(&self) -> Result<Box<dyn RevocationScope + '_>>;
	async fn begin_records(&self) -> Result<Box<dyn RevocationRecordScope + '_>>;
	/// Wake workers so Runs waiting on a closed approval continue.
	fn notify(&self);
}

/// A Document snapshot whose contents may have lost their retention basis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedSnapshot {
	pub id: Uuid,
	pub fetched_at: DateTime<Utc>,
	/// When the snapshot's Run ended, if it has.
	pub run_ended_at: Option<DateTime<Utc>>,
	/// The periods stored by the Run's Tenant, if any.
	pub tenant_rules: Option<RetentionRules>,
}

/// The snapshot contents the retention sweeper may purge.
#[async_trait]
pub trait SnapshotRetention: Send + Sync {
	/// Up to `limit` unpurged snapshots after `after` in identifier order whose
	/// Run ended at or before `ended_before` or that were retrieved at or
	/// before `retrieved_before`. Others cannot have lost their basis yet.
	async fn candidates(
		&self,
		after: Option<Uuid>,
		retrieved_before: DateTime<Utc>,
		ended_before: DateTime<Utc>,
		limit: u32,
	) -> Result<Vec<RetainedSnapshot>>;
	/// The shortest of each period any Tenant stores, if any Tenant stores
	/// periods.
	async fn shortest_tenant_periods(&self) -> Result<Option<SnapshotPeriods>>;
	/// Purge the contents of those `ids` still unpurged that `expired` holds
	/// for once they are read again with their Tenant's current periods, which
	/// no change can replace until the purge commits. Returns how many.
	async fn purge(
		&self,
		ids: &[Uuid],
		expired: &(dyn for<'a> Fn(&'a RetainedSnapshot) -> bool + Sync),
		at: DateTime<Utc>,
	) -> Result<u64>;
}

/// A research attempt whose Provider result is still kept, with the terms it
/// was retrieved under (ADR 0058).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedResult {
	pub attempt_id: Uuid,
	pub account_id: String,
	pub retrieved_at: DateTime<Utc>,
	pub retention_days: u16,
}

/// The Provider results the account-terms sweeper may purge, across every
/// Tenant.
#[async_trait]
pub trait ProviderResultRetention: Send + Sync {
	/// Record each termination not recorded yet. A recorded one is kept as
	/// it is, so removing or changing a declaration never undoes it.
	async fn record_terminations(&self, terminations: &[Termination]) -> Result<()>;
	/// Every recorded termination.
	async fn terminations(&self) -> Result<Vec<Termination>>;
	/// Up to `limit` attempts after `after` in identifier order whose Provider
	/// result is unpurged and that were retrieved at or before
	/// `retrieved_before` or under one of `accounts`. Others cannot have lost
	/// their basis yet.
	async fn candidates(
		&self,
		after: Option<Uuid>,
		retrieved_before: DateTime<Utc>,
		accounts: &[String],
		limit: u32,
	) -> Result<Vec<RetainedResult>>;
	/// Replace every stored copy of the Provider results of `attempts` with
	/// tombstones, keeping identities and digests, and mark them purged at
	/// `at` (ADR 0058). Returns how many attempts were purged.
	async fn purge(&self, attempts: &[Uuid], at: DateTime<Utc>) -> Result<u64>;
}
