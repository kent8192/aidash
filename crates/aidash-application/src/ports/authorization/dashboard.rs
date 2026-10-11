//! Account status writes retain their compare-and-update and authority transactions.
use crate::Result;
use aidash_domain::identity::dashboard::{Account, AccountState, SignIn};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct AccountPolicy {
	pub issuer: String,
	pub google: bool,
	pub tenant_bindings: Option<BTreeMap<String, String>>,
	pub gcip_providers: BTreeMap<String, Vec<String>>,
}

impl AccountPolicy {
	pub fn providers_for(&self, tenant: &str) -> Vec<String> {
		self.gcip_providers
			.get(tenant)
			.cloned()
			.unwrap_or_else(|| vec!["google.com".into(), "password".into()])
	}
	/// The one GCIP Tenant bound to `tenant`. Only a deployment with Tenant
	/// Bindings ties Registration Requests to a Tenant.
	pub fn bound_pool(&self, tenant: &str) -> Option<&str> {
		self.tenant_bindings
			.as_ref()?
			.iter()
			.find_map(|(pool, bound)| (bound == tenant).then_some(pool.as_str()))
	}
	pub fn require_sign_in_provider(&self, sign_in: &SignIn) -> Result<()> {
		if self.tenant_bindings.is_some() {
			let pool = sign_in
				.gcip_tenant
				.as_deref()
				.ok_or(crate::Error::Forbidden)?;
			let provider = sign_in
				.gcip_provider
				.as_ref()
				.ok_or(crate::Error::Forbidden)?;
			if !self.providers_for(pool).contains(provider) {
				return Err(crate::Error::Forbidden);
			}
		}
		Ok(())
	}
	pub fn require_identity(&self, issuer: &str, gcip_tenant: Option<&str>) -> Result<()> {
		if issuer != self.issuer {
			return Err(crate::Error::Forbidden);
		}
		if let Some(bindings) = &self.tenant_bindings {
			aidash_domain::identity::dashboard::bound_tenant(bindings, gcip_tenant)
				.ok_or(crate::Error::Forbidden)?;
		} else if gcip_tenant.is_some() {
			return Err(crate::Error::Forbidden);
		}
		Ok(())
	}
	pub fn require_mapping(
		&self,
		issuer: &str,
		gcip_tenant: Option<&str>,
		tenant: &str,
	) -> Result<()> {
		self.require_identity(issuer, gcip_tenant)?;
		if let Some(bindings) = &self.tenant_bindings
			&& aidash_domain::identity::dashboard::bound_tenant(bindings, gcip_tenant)
				!= Some(tenant)
		{
			return Err(crate::Error::Forbidden);
		}
		Ok(())
	}
}

#[async_trait]
pub trait AccountStatus: Send + Sync {
	async fn lookup(&self, subject: &str, gcip_tenant: Option<&str>) -> Result<AccountState>;
}

#[async_trait]
pub trait LoginAccounts: Send {
	async fn find(&mut self, issuer: &str, sign_in: &SignIn) -> Result<Option<Account>>;
	async fn register(&mut self, issuer: &str, sign_in: &SignIn) -> Result<Account>;
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
