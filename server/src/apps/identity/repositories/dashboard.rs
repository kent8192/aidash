//! Account state and recovery keep native ORM transactions and original Subject leases.
use crate::{
	apps::{
		execution::models::Run,
		identity::models::{DashboardExecutionOrigin, DashboardIdentity},
	},
	authorization::access::Access,
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::dashboard::{AccountPolicy, Accounts, StatusRecovery},
};
use aidash_domain::identity::dashboard::Account;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use uuid::Uuid;

pub(crate) struct Repository(pub(crate) Federation);

pub(crate) struct Login(pub(crate) reinhardt::db::orm::DatabaseConnection);
#[async_trait]
impl aidash_application::ports::authorization::dashboard::LoginAccounts for Login {
	async fn register(&mut self, issuer: &str, subject: &str) -> Result<Account> {
		Ok(account(
			DashboardIdentity::register(self.0, issuer, subject).await?,
		))
	}
}
struct Recovery {
	federation: Federation,
	lease: DatabaseConnectionLease,
}
pub(crate) fn account(row: DashboardIdentity) -> Account {
	Account {
		id: row.id,
		issuer: row.issuer,
		subject: row.subject,
		last_valid_at: row.last_valid_at,
		disabled_at: row.disabled_at,
	}
}

#[async_trait]
impl Accounts for Repository {
	fn policy(&self) -> Option<AccountPolicy> {
		self.0.config.oidc.as_ref().map(|config| AccountPolicy {
			issuer: config.issuer.clone(),
			google: config.is_google(),
		})
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn active(&self) -> Result<Vec<Account>> {
		let config = self.0.config.oidc.as_ref().ok_or_else(|| {
			aidash_application::Error::NotFound("dashboard sign-in is not configured".into())
		})?;
		let lease = self.0.store.orm_connection()?;
		Ok(
			DashboardIdentity::active(lease.handle(), config.session_idle_seconds)
				.await?
				.into_iter()
				.map(account)
				.collect(),
		)
	}
	async fn record_valid(&self, identity: Uuid, started: DateTime<Utc>) -> Result<()> {
		let lease = self.0.store.orm_connection()?;
		DashboardIdentity::record_valid(lease.handle(), identity, started)
			.await
			.map_err(Into::into)
	}
	async fn disable_if_current(
		&self,
		identity: Uuid,
		started: Option<DateTime<Utc>>,
	) -> Result<bool> {
		let lease = self.0.store.orm_connection()?;
		DashboardIdentity::disable_if_current(lease.handle(), identity, started)
			.await
			.map_err(Into::into)
	}
	async fn mark_waiting_disabled(&self, identity: Uuid) -> Result<()> {
		let lease = self.0.store.orm_connection()?;
		let mut connection = lease.handle();
		let ids = DashboardExecutionOrigin::status_waiting(&mut connection, identity).await?;
		Run::mark_identity_disabled(&mut connection, ids)
			.await
			.map_err(Into::into)
	}
	async fn recovery(&self) -> Result<Box<dyn StatusRecovery>> {
		Ok(Box::new(Recovery {
			federation: self.0.clone(),
			lease: self.0.store.orm_connection()?,
		}))
	}
}

#[async_trait]
impl StatusRecovery for Recovery {
	async fn account(&mut self, identity: Uuid) -> Result<Account> {
		Ok(account(
			DashboardIdentity::find(&mut self.lease.handle(), identity).await?,
		))
	}
	async fn waiting(&mut self, identity: Uuid) -> Result<Vec<Uuid>> {
		DashboardExecutionOrigin::status_waiting(&mut self.lease.handle(), identity)
			.await
			.map_err(Into::into)
	}
	async fn resume_original(&mut self, identity: Uuid, run: Uuid) -> Result<bool> {
		let Some(subject) =
			DashboardExecutionOrigin::original_subject(&mut self.lease.handle(), identity, run)
				.await?
		else {
			return Ok(false);
		};
		let Ok(access) = Access::begin(&self.federation.store, &subject).await else {
			return Ok(false);
		};
		let changed = Run::resume_identity_pause(self.lease.handle(), run).await;
		access.finish(changed).await.map_err(Into::into)
	}
	async fn restore(&mut self, identity: Uuid, started: DateTime<Utc>) -> Result<()> {
		DashboardIdentity::restore(&mut self.lease.handle(), identity, started)
			.await
			.map_err(Into::into)
	}
	fn notify(&self) {
		self.federation.notify.notify_waiters();
	}
}

pub(crate) struct Logout<'a>(pub(crate) &'a Federation);
#[async_trait]
impl aidash_application::ports::authorization::dashboard::BackchannelLogout for Logout<'_> {
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn verified_claims(&self, token: &str) -> Result<serde_json::Value> {
		let config = self.0.config.oidc.as_ref().ok_or_else(|| {
			aidash_application::Error::NotFound("dashboard sign-in is not configured".into())
		})?;
		aidash_integrations::oidc::verified_logout(
			&self.0.client,
			&config.issuer,
			&config.client_id,
			token,
		)
		.await
	}
	async fn revoke(
		&self,
		claims: &aidash_domain::identity::dashboard::LogoutClaims,
	) -> Result<()> {
		use sha2::{Digest, Sha256};
		let config = self.0.config.oidc.as_ref().ok_or_else(|| {
			aidash_application::Error::NotFound("dashboard sign-in is not configured".into())
		})?;
		let lease = self.0.store.orm_connection()?;
		crate::apps::identity::models::DashboardLogoutToken::revoke_sessions(
			lease.handle(),
			&config.issuer,
			claims.subject.as_deref(),
			claims.session.as_deref(),
			Sha256::digest(format!("{}:{}", config.issuer, claims.jti).as_bytes()).to_vec(),
			claims.expires_at,
		)
		.await
		.map_err(Into::into)
	}
}
