//! Account status writes retain their compare-and-update and authority transactions.
use crate::Result;
use aidash_domain::identity::dashboard::{Account, AccountState, SignIn};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use uuid::Uuid;

/// The issuers whose External Identities may sign in, and the Tenant each
/// issuer's identities may be restricted to.
pub trait Issuers: Send + Sync {
	/// Every issuer whose External Identities may sign in.
	fn admitted(&self) -> Vec<String>;
	fn admits(&self, issuer: &str) -> bool {
		self.admitted().iter().any(|admitted| admitted == issuer)
	}
	/// The only Tenant that External Identities of `issuer` may be mapped into.
	fn restriction(&self, issuer: &str) -> Option<String>;
	/// The issuer whose External Identities are restricted to `tenant`.
	fn restricted_issuer(&self, tenant: &str) -> Option<String>;
	/// Whether refusing an identity disables it at once. A restriction is
	/// policy, so removing one retires its identities; an issuer that is no
	/// longer configured retires its identities in the background instead.
	fn retires_refused(&self) -> bool {
		false
	}
}

/// One configured issuer, which admits only itself and restricts no identity.
pub struct Issuer(pub String);

impl Issuers for Issuer {
	fn admitted(&self) -> Vec<String> {
		vec![self.0.clone()]
	}
	fn admits(&self, issuer: &str) -> bool {
		issuer == self.0
	}
	fn restriction(&self, _: &str) -> Option<String> {
		None
	}
	fn restricted_issuer(&self, _: &str) -> Option<String> {
		None
	}
}

#[derive(Clone)]
pub struct AccountPolicy {
	pub issuers: Arc<dyn Issuers>,
	/// The Tenancy Rule that creates a Personal Tenant at sign-in.
	pub personal_tenants: bool,
	/// The Deployment loads Demo data (`[demo_data] enabled`), so a sign-in
	/// fulfils the Demo Reservations held for the identity's verified email.
	pub demo_data: bool,
}

impl AccountPolicy {
	/// Whether no restriction applies to identities of `issuer`, so the
	/// Tenancy Rules cover them.
	pub fn unrestricted(&self, issuer: &str) -> bool {
		self.issuers.restriction(issuer).is_none()
	}
	/// Whether signing in creates a Personal Tenant for an identity of `issuer`.
	pub fn provisions_personal(&self, issuer: &str) -> bool {
		self.personal_tenants && self.unrestricted(issuer)
	}
	pub fn require_identity(&self, issuer: &str) -> Result<()> {
		if !self.issuers.admits(issuer) {
			return Err(crate::Error::Forbidden);
		}
		Ok(())
	}
	pub fn require_mapping(&self, issuer: &str, tenant: &str) -> Result<()> {
		self.require_identity(issuer)?;
		if self
			.issuers
			.restriction(issuer)
			.is_some_and(|restricted| restricted != tenant)
		{
			return Err(crate::Error::Forbidden);
		}
		Ok(())
	}
}

#[async_trait]
pub trait AccountStatus: Send + Sync {
	async fn lookup(&self, issuer: &str, subject: &str) -> Result<AccountState>;
}

#[async_trait]
pub trait LoginAccounts: Send {
	async fn find(&mut self, sign_in: &SignIn) -> Result<Option<Account>>;
	async fn register(&mut self, sign_in: &SignIn) -> Result<Account>;
	/// Create a Personal Tenant once; never restore withdrawn authority.
	async fn provision_personal(&mut self, identity: Uuid) -> Result<()>;
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
	/// Every enabled Identity, including those without live sessions or runs.
	async fn identities(&self) -> Result<Vec<Account>>;
	async fn active(&self) -> Result<Vec<Account>>;
	/// Record freshness and revoke older sessions atomically; admitted work is untouched.
	async fn record_valid(
		&self,
		identity: Uuid,
		started: DateTime<Utc>,
		valid_since: Option<DateTime<Utc>>,
	) -> Result<()>;
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
