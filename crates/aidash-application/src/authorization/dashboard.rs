//! Ongoing runs and browser sessions share the same account validity deadline.
use crate::{
	Error, Result,
	ports::authorization::dashboard::{AccountStatus, Accounts},
};
use aidash_domain::identity::dashboard::{Account, SignIn, session_revoked};
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use uuid::Uuid;

pub const STATUS_FRESH_SECONDS: i64 = 300;
pub const STATUS_LIMIT_SECONDS: i64 = 900;

pub struct DashboardAuthority {
	pub accounts: Arc<dyn Accounts>,
	pub status: Arc<dyn AccountStatus>,
}
impl DashboardAuthority {
	pub fn configured(&self) -> bool {
		self.accounts.policy().is_some()
	}
	pub async fn admit_login(
		&self,
		scope: &mut dyn crate::ports::authorization::dashboard::LoginAccounts,
		sign_in: &SignIn,
	) -> Result<Account> {
		let policy = self
			.accounts
			.policy()
			.ok_or_else(|| Error::NotFound("dashboard sign-in is not configured".into()))?;
		if let Err(error) = policy.require_identity(&policy.issuer, sign_in.gcip_tenant.as_deref())
		{
			// A verified login reaches inactive identities that the background
			// refresh cannot see. Retire their authority before rejecting it.
			if (policy.tenant_bindings.is_some() || sign_in.gcip_tenant.is_some())
				&& let Some(account) = scope.find(&policy.issuer, sign_in).await?
			{
				self.disable(account.id, None).await?;
			}
			return Err(error);
		}
		let started = self.accounts.now();
		let status = if policy.google {
			None
		} else {
			Some(
				self.status
					.lookup(&sign_in.subject, sign_in.gcip_tenant.as_deref())
					.await?,
			)
		};
		if status.is_some_and(|status| {
			status.disabled || session_revoked(sign_in.auth_time, status.valid_since)
		}) {
			if let Some(account) = scope.find(&policy.issuer, sign_in).await?
				&& let Some(status) = status
			{
				if status.disabled {
					self.disable(account.id, Some(started)).await?;
				} else if account.disabled_at.is_none() {
					self.accounts
						.record_valid(account.id, started, status.valid_since)
						.await?;
				}
			}
			return Err(Error::Forbidden);
		}
		let account = scope.register(&policy.issuer, sign_in).await?;
		if account.disabled_at.is_some() {
			return Err(Error::Forbidden);
		}
		if let Some(status) = status {
			self.accounts
				.record_valid(account.id, started, status.valid_since)
				.await?;
		}
		Ok(account)
	}
	pub async fn account_valid(&self, account: &Account) -> Result<()> {
		if account.disabled_at.is_some() {
			return Err(Error::Forbidden);
		}
		let policy = self
			.accounts
			.policy()
			.ok_or_else(|| Error::NotFound("dashboard sign-in is not configured".into()))?;
		if policy
			.require_identity(&account.issuer, account.gcip_tenant.as_deref())
			.is_err()
		{
			// GCIP Binding removal takes effect durably at this boundary. Keep
			// generic OIDC's request rejection and background issuer retirement.
			if policy.tenant_bindings.is_some() || account.gcip_tenant.is_some() {
				self.disable(account.id, None).await?;
			}
			return Err(Error::Forbidden);
		}
		if policy.google {
			return Ok(());
		}
		let now = self.accounts.now();
		if account
			.last_valid_at
			.is_some_and(|last| last > now - Duration::seconds(STATUS_FRESH_SECONDS))
		{
			return Ok(());
		}
		// The deadline begins before provider IO, including a slow response body.
		match self
			.status
			.lookup(&account.subject, account.gcip_tenant.as_deref())
			.await
		{
			Ok(status) if !status.disabled => {
				self.accounts
					.record_valid(account.id, now, status.valid_since)
					.await
			}
			Ok(_) => {
				self.disable(account.id, Some(now)).await?;
				Err(Error::Forbidden)
			}
			Err(_)
				if account
					.last_valid_at
					.is_some_and(|last| last > now - Duration::seconds(STATUS_LIMIT_SECONDS)) =>
			{
				Ok(())
			}
			Err(_) => Err(Error::IdentityStatusUnavailable),
		}
	}
	pub async fn disable(&self, identity: Uuid, started: Option<DateTime<Utc>>) -> Result<()> {
		if self.accounts.disable_if_current(identity, started).await? {
			self.accounts.mark_waiting_disabled(identity).await?;
		}
		Ok(())
	}
	pub async fn resume(&self, identity: Uuid) -> Result<()> {
		let mut scope = self.accounts.recovery().await?;
		let account = scope.account(identity).await?;
		if account.disabled_at.is_some()
			|| account.last_valid_at.is_none_or(|last| {
				last <= self.accounts.now() - Duration::seconds(STATUS_FRESH_SECONDS)
			}) {
			return Ok(());
		}
		for run in scope.waiting(identity).await? {
			if scope.resume_original(identity, run).await? {
				scope.notify();
			}
		}
		Ok(())
	}
	pub async fn active(&self) -> Result<Vec<Account>> {
		self.accounts.active().await
	}
	pub async fn restore(&self, identity: Uuid) -> Result<()> {
		let policy = self
			.accounts
			.policy()
			.ok_or_else(|| Error::NotFound("dashboard sign-in is not configured".into()))?;
		let mut scope = self.accounts.recovery().await?;
		let account = scope.account(identity).await?;
		if policy
			.require_identity(&account.issuer, account.gcip_tenant.as_deref())
			.is_err() || account.disabled_at.is_none()
		{
			return Err(Error::Conflict(
				"identity is not disabled for the configured issuer".into(),
			));
		}
		let started = self.accounts.now();
		if self
			.status
			.lookup(&account.subject, account.gcip_tenant.as_deref())
			.await?
			.disabled
		{
			return Err(Error::Forbidden);
		}
		scope.restore(identity, started).await
	}
	pub async fn refresh(&self, account: Account) {
		let policy = self.accounts.policy();
		if policy.as_ref().is_some_and(|policy| policy.google) {
			return;
		}
		if policy.as_ref().is_some_and(|policy| {
			account.issuer != policy.issuer
				|| policy
					.require_identity(&account.issuer, account.gcip_tenant.as_deref())
					.is_err()
		}) {
			if let Err(error) = self.disable(account.id, None).await {
				tracing::warn!(identity_id=%account.id, %error, "old-issuer identity could not be disabled");
			}
			return;
		}
		match self.account_valid(&account).await {
			Ok(()) => {
				if let Err(error) = self.resume(account.id).await {
					tracing::warn!(identity_id=%account.id, %error, "status recovery check failed");
				}
			}
			Err(error) if !matches!(error, Error::Forbidden | Error::IdentityStatusUnavailable) => {
				tracing::warn!(identity_id=%account.id, "issuer status refresh did not establish validity");
			}
			Err(_) => {}
		}
	}
}

pub async fn backchannel_logout(
	scope: &dyn crate::ports::authorization::dashboard::BackchannelLogout,
	token: &str,
) -> Result<()> {
	let verified = scope.verified_claims(token).await?;
	let claims =
		aidash_domain::identity::dashboard::logout_claims(&verified, scope.now().timestamp())?;
	scope.revoke(&claims).await
}

#[cfg(test)]
mod tests;
