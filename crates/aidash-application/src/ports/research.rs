//! Ports of the host-owned research contract. Provider packages implement
//! [`SearchProvider`] or [`PageProvider`]; the Node implements the evidence
//! store and the Web disclosure gate once for every provider.
use crate::Result;
use aidash_domain::research::{
	Failure, Operation,
	account::{AccountProfile, CostAttribution},
	citations::EvidenceRef,
	document::{ExtractedPage, Passage},
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
	/// The Search account holder's current account terms, or `None` for a
	/// provider that sends nothing outside the Node.
	fn account(&self) -> std::result::Result<Option<AccountProfile>, Failure>;
	async fn search(&self, query: &SearchQuery) -> std::result::Result<ProviderResults, Failure>;
}

/// A Page reading provider. It must refuse non-public destinations itself,
/// including addresses a public name resolves to at dispatch.
#[async_trait]
pub trait PageProvider: Send + Sync {
	/// Literal address checks performed before any disclosure or attempt.
	fn admit(&self, url: &Url) -> std::result::Result<(), Failure>;
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure>;
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
	/// Whether the Run's Research evidence was revoked.
	async fn revoked(&self, run: Uuid) -> Result<bool>;
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
	/// Source records in candidate order.
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
	async fn record_snapshot(
		&self,
		run: Uuid,
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
	/// A subject must be permitted `action` on its Tenant's revocation records;
	/// a Deployment Administrator needs no Tenant policy.
	async fn require(&mut self, tenant: &str, action: &str) -> Result<()>;
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
	/// Run ended before `ended_before` or that were retrieved before
	/// `retrieved_before`. Others cannot have lost their basis yet.
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
