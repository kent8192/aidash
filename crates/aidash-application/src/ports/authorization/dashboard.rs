//! Account status writes retain their compare-and-update and authority transactions.
use crate::Result;
use aidash_domain::identity::dashboard::Account;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub struct AccountPolicy {
	pub issuer: String,
	pub google: bool,
}

#[async_trait]
pub trait AccountStatus: Send + Sync {
	async fn enabled(&self, subject: &str) -> Result<bool>;
}

#[async_trait]
pub trait LoginAccounts: Send {
	async fn register(&mut self, issuer: &str, subject: &str) -> Result<Account>;
}

/// Retain one native connection lease through the complete recovery pass.
#[async_trait]
pub trait StatusRecovery: Send {
	async fn account(&mut self, identity: Uuid) -> Result<Account>;
	async fn waiting(&mut self, identity: Uuid) -> Result<Vec<Uuid>>;
	/// Missing or revoked original credentials skip this run. A successful resume
	/// keeps the fresh original Subject authority until its update commits.
	async fn resume_original(&mut self, identity: Uuid, run: Uuid) -> Result<bool>;
	async fn restore(&mut self, identity: Uuid, started: DateTime<Utc>) -> Result<()>;
	fn notify(&self);
}

#[async_trait]
pub trait Accounts: Send + Sync {
	fn policy(&self) -> Option<AccountPolicy>;
	fn now(&self) -> DateTime<Utc>;
	async fn active(&self) -> Result<Vec<Account>>;
	async fn record_valid(&self, identity: Uuid, started: DateTime<Utc>) -> Result<()>;
	async fn disable_if_current(
		&self,
		identity: Uuid,
		started: Option<DateTime<Utc>>,
	) -> Result<bool>;
	async fn mark_waiting_disabled(&self, identity: Uuid) -> Result<()>;
	async fn recovery(&self) -> Result<Box<dyn StatusRecovery>>;
}

#[async_trait]
pub trait BackchannelLogout: Send + Sync {
	fn now(&self) -> DateTime<Utc>;
	async fn verified_claims(&self, token: &str) -> Result<serde_json::Value>;
	/// Store the replay identity and revoke its sessions in the same transaction.
	async fn revoke(&self, claims: &aidash_domain::identity::dashboard::LogoutClaims)
	-> Result<()>;
}
