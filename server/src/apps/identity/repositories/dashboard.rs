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
	async fn find(
		&mut self,
		issuer: &str,
		sign_in: &aidash_domain::identity::dashboard::SignIn,
	) -> Result<Option<Account>> {
		use reinhardt::db::orm::Model;
		Ok(DashboardIdentity::objects()
			.filter(DashboardIdentity::field_issuer().eq(issuer.to_owned()))
			.filter(
				DashboardIdentity::field_gcip_tenant()
					.eq(sign_in.gcip_tenant.clone().unwrap_or_default()),
			)
			.filter(DashboardIdentity::field_subject().eq(sign_in.subject.clone()))
			.all_with_db(&mut self.0)
			.await
			.map_err(crate::Error::from)?
			.pop()
			.map(account))
	}

	async fn register(
		&mut self,
		issuer: &str,
		sign_in: &aidash_domain::identity::dashboard::SignIn,
	) -> Result<Account> {
		Ok(account(
			DashboardIdentity::register(self.0, issuer, sign_in).await?,
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
		gcip_tenant: (!row.gcip_tenant.is_empty()).then_some(row.gcip_tenant),
		valid_since: row.valid_since,
		last_valid_at: row.last_valid_at,
		disabled_at: row.disabled_at,
	}
}

#[async_trait]
impl Accounts for Repository {
	fn policy(&self) -> Option<AccountPolicy> {
		self.0.config.dashboard_policy()
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn identities(&self) -> Result<Vec<Account>> {
		use reinhardt::db::orm::Model;
		let lease = self.0.store.orm_connection()?;
		Ok(DashboardIdentity::objects()
			.filter(DashboardIdentity::field_disabled_at().is_null())
			.all_with_db(&mut lease.handle())
			.await
			.map_err(crate::Error::from)?
			.into_iter()
			.map(account)
			.collect())
	}
	async fn active(&self) -> Result<Vec<Account>> {
		let config = self.0.config.dashboard_session().ok_or_else(|| {
			aidash_application::Error::NotFound("dashboard sign-in is not configured".into())
		})?;
		let lease = self.0.store.orm_connection()?;
		DashboardIdentity::expire_registrations(lease.handle()).await?;
		Ok(
			DashboardIdentity::active(lease.handle(), config.session_idle_seconds)
				.await?
				.into_iter()
				.map(account)
				.collect(),
		)
	}
	async fn record_valid(
		&self,
		identity: Uuid,
		started: DateTime<Utc>,
		valid_since: Option<DateTime<Utc>>,
	) -> Result<()> {
		let lease = self.0.store.orm_connection()?;
		DashboardIdentity::record_valid(lease.handle(), identity, started, valid_since)
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
