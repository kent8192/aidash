//! Fail-closed boundaries for decision authority, physical requests and durable evidence.
use crate::Result;
use aidash_domain::{context::Context, decision::*};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// A Node-owned disclosure adapter constructs these views after source/provenance checks.
/// Unknown or forbidden events are absent and remain protected in the original history.
#[derive(Debug, Clone, Default)]
pub struct Disclosure {
	pub goal: Option<String>,
	pub conversation: BTreeMap<usize, String>,
	pub tool_inputs: BTreeMap<usize, Value>,
	pub sources: Vec<SourcePin>,
	pub source_expiry: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Approval {
	pub restrictions: Restrictions,
	/// Full-state retention must already be explicitly authorized for this tenant/Node.
	pub state_retention: policy::StateRetention,
}

#[async_trait]
pub trait DecisionAuthority: Send + Sync {
	/// Checks exact catalog/configuration pins, decision.invoke and permitted disclosure.
	/// Called again before each physical dispatch, including Shadow and retries.
	async fn check(
		&self,
		boundary: &Boundary,
		decider: &DeciderPin,
		sources: &[SourcePin],
	) -> Result<Approval>;
	/// Requires Run access, decision.evidence.read and current authority for every source.
	/// Full state additionally requires decision.state.read. No execution is authorized.
	async fn read(&self, evidence: &Evidence, full_state: bool) -> Result<()>;
}

/// The transport must send precisely these bytes, with no automatic retry/redirect.
#[derive(Debug, Clone)]
pub struct PreparedRequest {
	pub body: Vec<u8>,
	pub questions: Questions,
}
impl PreparedRequest {
	pub fn digest(&self) -> String {
		use sha2::{Digest, Sha256};
		format!("sha256:{:x}", Sha256::digest(&self.body))
	}
}

/// Safe dispatch categories; raw responses and transport errors never enter evidence.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
	#[error(transparent)]
	ProviderFailure(#[from] crate::Error),
	#[error("invalid decision answers")]
	InvalidAnswers,
}

pub trait DecisionProvider: Send + Sync {
	/// Opaque identity of this concrete adapter in the trusted Node Provider catalog.
	/// It must match the admitted implementation, independently of the protocol/config.
	fn implementation_id(&self) -> &str;
	fn configuration_digest(&self) -> Result<String>;
	/// Split only under factual constraints of the pinned external provider.
	/// All question IDs must occur exactly once; no state/candidate truncation is allowed.
	fn plan(&self, state: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>>;
	/// Credential and request construction failures happen before reserving a physical call.
	/// The returned transport retains the resolved credentials and exact request;
	/// dispatch must not repeat fallible construction or credential resolution.
	/// Credentials never enter the planned body or error messages.
	fn prepare(&self, request: &PreparedRequest) -> Result<Box<dyn PreparedDispatch + '_>>;
}

/// A fully constructed transport consumed by one reserved physical attempt.
#[async_trait]
pub trait PreparedDispatch: Send {
	/// One physical HTTP attempt, with strict model/type/coverage/finite-probability checks.
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRecord {
	pub attempt: Uuid,
	pub decision: Uuid,
	pub boundary: Boundary,
	pub decider: DeciderPin,
	pub request_digest: String,
	pub state_digest: String,
	pub questions: Vec<String>,
	pub mode: Mode,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchPermit {
	pub record: DispatchRecord,
	pub owner_receipts: BTreeMap<String, Uuid>,
}

#[derive(Debug, Clone)]
pub struct RetainedState {
	pub id: Uuid,
	pub digest: String,
	pub expires_at: DateTime<Utc>,
	pub state: Value,
}

#[async_trait]
pub trait DecisionJournal: Send + Sync {
	/// The persistence clock, also used to discard state that expired during dispatch.
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	/// Every owner atomically checks pinned/current allowances and durably charges all
	/// local generated ancestors. Waits for all remote owner receipts and commits the
	/// dispatch record before returning. Concurrent reservations must not overspend.
	/// A returned permit stays charged even if the worker dies before transport returns.
	async fn reserve(&self, record: &DispatchRecord) -> Result<DispatchPermit>;
	/// Stores validated answers or safe failure/uncertainty without raw provider bodies.
	/// Persisted answered attempts are the only possible recovery source.
	async fn finish_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> Result<()>;
	/// Rechecks lease, input/run revisions and exact pins (including the Node Provider
	/// implementation) inside the commit boundary.
	/// Applying context or retaining state also requires current source/invocation
	/// authority. Revocation or invalid approval must still permit a fenced rejection
	/// without context or full state, preserving dispatched attempts for recovery.
	/// Applied context, evidence and optional state commit atomically; errors must
	/// leave the authoritative Run unchanged.
	/// Shadow/rejected records use None and must never replace execution context.
	/// At the atomic write, compare state.expires_at with the persistence clock. If
	/// elapsed, omit the state bytes and persist an Expired reference in the evidence.
	async fn commit(
		&self,
		evidence: &Evidence,
		context: Option<&Context>,
		state: Option<&RetainedState>,
	) -> Result<()>;
	/// Retention cleanup deletes state bytes and atomically leaves an expired marker.
	async fn expire_states(&self, now: DateTime<Utc>) -> Result<usize>;
	async fn append_outcome(&self, outcome: &OutcomeLink) -> Result<()>;
}
