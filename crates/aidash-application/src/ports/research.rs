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
	search::{ProviderResults, SearchQuery, SourceCandidate},
};
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
	/// The Search service operator's current account terms, or `None` for a
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
}

/// Run-scoped Research evidence. Every read is limited to the given Run; an
/// identifier of another Run is reported as absent.
#[async_trait]
pub trait EvidenceStore: Send + Sync {
	async fn attempt(&self, run: Uuid, key: &str) -> Result<Option<AttemptState>>;
	async fn usage(&self, run: Uuid) -> Result<RunUsage>;
	/// Charges attributed to `account` since `since`, including dispatched
	/// attempts whose outcome is still unknown.
	async fn account_spend(&self, account: &str, since: DateTime<Utc>) -> Result<u64>;
	async fn reserve(&self, run: Uuid, reservation: &Reservation) -> Result<Reserved>;
	/// Settles a dispatched attempt exactly once; settling a missing or
	/// already settled attempt is a `Conflict`.
	async fn settle(
		&self,
		run: Uuid,
		key: &str,
		outcome: AttemptOutcome,
		result: &Value,
	) -> Result<()>;
	/// Source records in candidate order.
	async fn record_sources(
		&self,
		run: Uuid,
		key: &str,
		sources: &[SourceCandidate],
		max_sources: u32,
	) -> Result<Vec<SourceRecord>>;
	async fn source(&self, run: Uuid, id: Uuid) -> Result<Option<SourceRecord>>;
	/// A new Document snapshot of the Source record `source`. Under the Run's
	/// lock, the snapshot is refused with `SnapshotLimit` when it would exceed
	/// the Run's snapshot limits.
	async fn record_snapshot(
		&self,
		run: Uuid,
		source: Uuid,
		requested_url: &str,
		page: &ExtractedPage,
		limits: &Limits,
	) -> Result<std::result::Result<DocumentSnapshot, Failure>>;
	async fn snapshot(&self, run: Uuid, id: Uuid) -> Result<Option<DocumentSnapshot>>;
	async fn record_fragment(
		&self,
		run: Uuid,
		document: &DocumentSnapshot,
		passage: &Passage,
	) -> Result<EvidenceFragment>;
	/// The subset of `references` delivered to this Run.
	async fn fragments(
		&self,
		run: Uuid,
		references: &[EvidenceRef],
	) -> Result<Vec<EvidenceFragment>>;
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
	/// An eligible person must decide first. The operator waits on this approval.
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
