//! Ports for the internal streaming gateway; authority and counters stay durable.
use crate::{Result, tenant_secrets::SecretRoute};
use aidash_domain::egress::ExternalService;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use std::sync::Arc;
use uuid::Uuid;

pub struct Admission {
	pub gateway_origin: String,
	pub destination: aidash_domain::keyless::Destination,
	pub request: Uuid,
	/// The operation or Live preview owner; absent for a Scratch Space call.
	pub operation: Option<Uuid>,
	/// Set when the token is a session token; the request record carries it.
	pub session: Option<Uuid>,
	pub service: String,
	pub definition: ExternalService,
	pub expires_at: DateTime<Utc>,
	/// The value a self-hosted gateway attaches itself. A brokered request
	/// never carries it (ADR 0095).
	pub credential: Option<SecretString>,
	/// Where a request for a secret-bearing External service goes when a Tenant
	/// Secret broker is configured. The broker attaches the value.
	pub route: Option<SecretRoute>,
	pub meter: Arc<dyn Meter>,
}

#[derive(Clone, Copy)]
pub enum Direction {
	Sent,
	Received,
}

/// What one request cost at the gateway, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestTimes {
	/// From the end of admission to the end of the response. A response that
	/// ends its own stream includes its final debit, unless that debit failed
	/// and was left to the background; one that stops at its `Content-Length`
	/// does not.
	pub duration_ms: u64,
	/// The gateway's admission call: the token, the authority's recheck, the
	/// budget locks and the record's first write.
	pub admission_ms: u64,
	/// From the end of the response until its final debit settled, including a
	/// first attempt that failed and the retries after it, or until the debit
	/// was given up (`egress_debit_abandoned`). Bytes kept when a debit failed in
	/// the middle of a stream, which stops the stream and is charged in the
	/// background, may settle later and are not counted.
	pub settle_ms: u64,
}

#[async_trait]
pub trait Meter: Send + Sync {
	/// Recheck authority even when no body bytes are flowing.
	async fn check_active(&self) -> Result<()> {
		Ok(())
	}
	/// Atomically adds body bytes the gateway has streamed, in batches, to the
	/// request record and to the operation's and this service's budgets. The
	/// bytes are always committed, even past a limit. Afterwards the call
	/// reports why the stream must stop: `Conflict(LIMIT_EXCEEDED)` when a
	/// budget is exhausted, `Conflict(OPERATION_EXPIRED)` when the operation or
	/// session ended or expired, and `Forbidden` when its authority was revoked
	/// (see [`aidash_domain::egress::LIMIT_EXCEEDED`]). Any other error means
	/// the bytes were not committed, and the caller charges them again.
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()>;
	/// Completes the metadata-only request record, including failures and partial bodies.
	async fn finish(&self, status: u16, times: RequestTimes, error: Option<&str>) -> Result<()>;
}

#[async_trait]
pub trait Authority: Send + Sync {
	/// Rechecks the operation, Run, Tenant, pinned Binding and live Catalog Approval.
	/// The token reaches only this operation's services. `path` excludes the query;
	/// `query_digest` is the only query material admitted into the audit record.
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		query_digest: &str,
	) -> Result<Admission>;
}

/// Compare stored opaque credentials without early exit on matching bytes.
/// Credential length is public; issuers should use fixed-length random tokens.
pub fn credentials_match(expected: &str, provided: &str) -> bool {
	use subtle::ConstantTimeEq;
	bool::from(expected.as_bytes().ct_eq(provided.as_bytes()))
}

/// Limits for one gateway instance and scope tunnel.
#[derive(Clone, Copy, Debug)]
pub struct TunnelLimits {
	/// Host connections one Scratch Space may have open at once. No setting
	/// changes it, so a Tenant's own limit is checked against this default.
	pub concurrent: usize,
	pub idle_timeout: std::time::Duration,
	pub max_lifetime: std::time::Duration,
}
impl Default for TunnelLimits {
	fn default() -> Self {
		Self {
			concurrent: 32,
			idle_timeout: std::time::Duration::from_secs(300),
			max_lifetime: std::time::Duration::from_secs(86400),
		}
	}
}

/// Settings resolved from a revocable Execution Pod proxy credential.
#[derive(Clone, Debug)]
pub struct ScopeSettings {
	pub id: aidash_domain::egress::ScopeId,
	pub allowed_hosts: aidash_domain::egress::AllowedHosts,
	pub budget: aidash_domain::egress::DailyBudget,
	pub anomaly: aidash_domain::egress::AnomalyRule,
	pub operator_allows_all: bool,
	pub tunnel_limits: TunnelLimits,
}

/// Metadata for a completed connection; URL paths and bodies are never recorded.
#[derive(Clone, Debug)]
pub struct ConnectionRecord {
	/// Stable identity permits retries and updates to the same live record.
	pub id: Uuid,
	pub scope: aidash_domain::egress::ScopeId,
	pub host: String,
	pub port: u16,
	pub sent_bytes: u64,
	pub received_bytes: u64,
	pub duration_ms: u64,
	pub outcome: String,
	/// The code the client received, for a connection that did not complete.
	pub error: Option<String>,
	/// How long it waited for a Host connection limit; `None` until known.
	pub wait_ms: Option<u64>,
}

/// Supplies live Scratch Space authority to the durable egress adapter.
/// Implementations verify credential authenticity, expiry and revocation, bind
/// it to the Tenant and Node, compare opaque credentials using `credentials_match`
/// (or a constant-time cryptographic verification), and reload the active Space and current Tenant
/// policy for an already authenticated scope. Host settings must be narrowed
/// by the current Tenant allowlist and the operator's Offering decision.
#[async_trait]
pub trait ScopeAuthority: Send + Sync {
	async fn authenticate(&self, credential: &str) -> Result<ScopeSettings>;
	async fn current(&self, scope: &aidash_domain::egress::ScopeId) -> Result<ScopeSettings>;
	/// The returned meter records request metadata and bytes; scope budgets are
	/// charged by the durable adapter, not by the authority's meter.
	async fn admit_external(
		&self,
		credential: &str,
		scope: &aidash_domain::egress::ScopeId,
		service: &str,
		method: &str,
		path: &str,
		query_digest: &str,
	) -> Result<Admission>;
}

/// Durable scope authority. Implementations must serialize admission, suspension,
/// and budget debits across all gateway instances. Debits are made before bytes
/// leave the gateway and may be reserved in batches; unused reservations must be
/// returned on completion. Each debit is associated with its UTC day.
#[async_trait]
pub trait EgressScopes: Send + Sync {
	async fn resolve(&self, credential: &str) -> Result<ScopeSettings>;
	/// Admit an External service made available to this scope. Implementations
	/// recheck the scope, service and daily limits before attaching any secret.
	async fn admit_external(
		&self,
		credential: &str,
		service: &str,
		method: &str,
		path: &str,
		query_digest: &str,
	) -> Result<Admission>;
	/// Atomically checks suspension, counts a new connection in the current
	/// minute, and suspends and notifies the operator on anomaly.
	async fn admit_connection(&self, scope: &ScopeSettings) -> Result<()>;
	/// Reauthenticate the credential, fail on suspension and return current policy.
	async fn check_active(&self, scope: &ScopeSettings, credential: &str) -> Result<ScopeSettings>;
	/// Atomically rejects suspended scopes and reserves bytes against the daily budget.
	async fn debit(
		&self,
		scope: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: chrono::NaiveDate,
	) -> Result<()>;
	async fn refund(
		&self,
		scope: &ScopeSettings,
		direction: Direction,
		bytes: u64,
		day: chrono::NaiveDate,
	) -> Result<()>;
	/// Idempotently inserts or updates a connection by record ID.
	async fn record(&self, record: ConnectionRecord) -> Result<()>;
	async fn lift_suspension(&self, scope: &aidash_domain::egress::ScopeId) -> Result<()>;
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
