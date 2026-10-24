//! Atomic authentication lifecycle storage and online admission ports.
use aidash_domain::accounts::{Account, ExternalIdentity, FreshProof, ProductSession};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Every call is a new protected-operation boundary; no positive-result cache.
/// Verify the returned original source/session provenance, not merely success.
/// Unreachable authority maps to `IdentityStatusUnavailable`, never fallback.
#[async_trait]
pub trait Authority: Send + Sync {
	async fn validate_interactive(
		&self,
		source_identity: Uuid,
		product_session: Uuid,
	) -> crate::Result<()>;
	async fn validate_origin(&self, source_identity: Uuid) -> crate::Result<()>;
	/// Capture only for a newly admitted Run or explicit authorized manual resume.
	async fn origin_binding(
		&self,
		source_identity: Uuid,
	) -> crate::Result<aidash_domain::accounts::DurableOrigin>;
	/// An automatic retry/resume must retain this admission's suspension epoch.
	async fn validate_bound_origin(
		&self,
		origin: &aidash_domain::accounts::DurableOrigin,
	) -> crate::Result<()>;
}

/// Implementations serialize concurrent mutations affecting the same Account,
/// Identity, proof and Session, including first-assignment races. Reads must see
/// completed stops; stale replicas cannot authorize a later operation.
#[async_trait]
pub trait Repository: Send + Sync {
	async fn begin(&self) -> crate::Result<Box<dyn Scope>>;
}
/// All writes, proof consumption and session revocations are one transaction.
/// Dropping without commit rolls them all back. Identity save MUST preserve
/// issuer/subject and an existing first_account; no adapter may reassign history.
#[async_trait]
pub trait Scope: Send {
	async fn account(&mut self, id: Uuid) -> crate::Result<Option<Account>>;
	async fn identity(&mut self, id: Uuid) -> crate::Result<Option<ExternalIdentity>>;
	async fn session(&mut self, id: Uuid) -> crate::Result<Option<ProductSession>>;
	async fn save_account(&mut self, account: &Account) -> crate::Result<()>;
	async fn save_identity(&mut self, identity: &ExternalIdentity) -> crate::Result<()>;
	async fn save_session(&mut self, session: &ProductSession) -> crate::Result<()>;
	/// A proof is single-use; it was stored only after explicit reauthentication.
	async fn consume_proof(&mut self, id: Uuid) -> crate::Result<Option<FreshProof>>;
	async fn revoke_identity_sessions(
		&mut self,
		identity: Uuid,
		now: DateTime<Utc>,
	) -> crate::Result<()>;
	async fn revoke_account_sessions(
		&mut self,
		account: Uuid,
		now: DateTime<Utc>,
	) -> crate::Result<()>;
	async fn commit(self: Box<Self>) -> crate::Result<()>;
}
