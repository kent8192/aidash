//! Account state and recovery keep native ORM transactions and original Subject leases.
use crate::database::native::Pool;
use crate::{
	apps::{
		execution::models::Run,
		identity::models::{
			DashboardExecutionOrigin, DashboardIdentity, DemoReservation, Fulfilment,
		},
	},
	authorization::access::Access,
	federation::Federation,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::{
	Result,
	ports::authorization::dashboard::{AccountPolicy, Accounts, StatusRecovery},
};
use aidash_domain::identity::dashboard::Account;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub(crate) struct Repository(pub(crate) Federation);

/// External Identities and sessions are Deployment-wide, so these ports work
/// in the no-Tenant scope; a personal Tenant is provisioned in its own scope.
pub(crate) struct Login(pub(crate) Pool, pub(crate) String);
#[async_trait]
impl aidash_application::ports::authorization::dashboard::LoginAccounts for Login {
	async fn fulfil_demo_reservations(&mut self, identity: Uuid, email: &str) -> Result<()> {
		let policy = self.0.dashboard_policy();
		// Reservations are Tenant-owned: this Deployment-scope read only finds
		// them, and each is spent in its own Tenant's scope and transaction, so
		// one that cannot be spent leaves the others to be.
		for reservation in DemoReservation::unspent(&self.0, &email.to_lowercase()).await? {
			let outcome = DemoReservation::fulfil(
				&self.0,
				policy.as_ref(),
				&reservation.tenant,
				reservation.id,
				identity,
			)
			.await?;
			if let Fulfilment::Skipped(reason) = outcome {
				tracing::debug!(%identity, tenant = %reservation.tenant, ?reason, "Demo Reservation not spent");
			}
		}
		Ok(())
	}
	async fn provision_personal(&mut self, identity: Uuid) -> Result<()> {
		super::personal_tenant::provision(&self.0, &self.1, identity)
			.await
			.map_err(Into::into)
	}
	async fn find(
		&mut self,
		sign_in: &aidash_domain::identity::dashboard::SignIn,
	) -> Result<Option<Account>> {
		use reinhardt::db::orm::Model;
		let rows = self
			.0
			.atomic(&TenancyScope::NoTenant, async |tx| {
				Ok(DashboardIdentity::objects()
					.filter(DashboardIdentity::field_issuer().eq(sign_in.issuer.clone()))
					.filter(DashboardIdentity::field_subject().eq(sign_in.subject.clone()))
					.all_with_db(tx)
					.await?)
			})
			.await?;
		Ok(rows.into_iter().last().map(account))
	}

	async fn register(
		&mut self,
		sign_in: &aidash_domain::identity::dashboard::SignIn,
	) -> Result<Account> {
		Ok(account(
			DashboardIdentity::register(&self.0, sign_in).await?,
		))
	}
}
struct Recovery {
	federation: Federation,
}

/// Status recovery follows one External Identity's Runs into every Tenant.
fn identity_status() -> TenancyScope {
	TenancyScope::Deployment(DeploymentScopeReason::IdentityStatus)
}
pub(crate) fn account(row: DashboardIdentity) -> Account {
	Account {
		id: row.id,
		issuer: row.issuer,
		subject: row.subject,
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
		let rows = self
			.0
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				Ok(DashboardIdentity::objects()
					.filter(DashboardIdentity::field_disabled_at().is_null())
					.all_with_db(tx)
					.await?)
			})
			.await?;
		Ok(rows.into_iter().map(account).collect())
	}
	async fn active(&self) -> Result<Vec<Account>> {
		let config = self.0.config.dashboard_session().ok_or_else(|| {
			aidash_application::Error::NotFound("dashboard sign-in is not configured".into())
		})?;
		let pool = &self.0.store.pool;
		DashboardIdentity::enforce_display_retention(pool, Some(config.session_idle_seconds))
			.await?;
		Ok(DashboardIdentity::active(pool, config.session_idle_seconds)
			.await?
			.into_iter()
			.map(account)
			.collect())
	}
	async fn record_valid(
		&self,
		identity: Uuid,
		started: DateTime<Utc>,
		valid_since: Option<DateTime<Utc>>,
	) -> Result<()> {
		DashboardIdentity::record_valid(&self.0.store.pool, identity, started, valid_since)
			.await
			.map_err(Into::into)
	}
	async fn disable_if_current(
		&self,
		identity: Uuid,
		started: Option<DateTime<Utc>>,
	) -> Result<bool> {
		DashboardIdentity::disable_if_current(&self.0.store.pool, identity, started)
			.await
			.map_err(Into::into)
	}
	async fn mark_waiting_disabled(&self, identity: Uuid) -> Result<()> {
		self.0
			.store
			.pool
			.atomic(&identity_status(), async |tx| {
				let ids = DashboardExecutionOrigin::status_waiting(tx, identity).await?;
				Run::mark_identity_disabled(tx, ids).await
			})
			.await
			.map_err(Into::into)
	}
	async fn recovery(&self) -> Result<Box<dyn StatusRecovery>> {
		Ok(Box::new(Recovery {
			federation: self.0.clone(),
		}))
	}
}

#[async_trait]
impl StatusRecovery for Recovery {
	async fn account(&mut self, identity: Uuid) -> Result<Account> {
		let row = self
			.federation
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::find(tx, identity).await
			})
			.await?;
		Ok(account(row))
	}
	async fn waiting(&mut self, identity: Uuid) -> Result<Vec<Uuid>> {
		self.federation
			.store
			.pool
			.atomic(&identity_status(), async |tx| {
				DashboardExecutionOrigin::status_waiting(tx, identity).await
			})
			.await
			.map_err(Into::into)
	}
	async fn resume_original(&mut self, identity: Uuid, run: Uuid) -> Result<bool> {
		let pool = &self.federation.store.pool;
		let Some(subject) = pool
			.atomic(&identity_status(), async |tx| {
				DashboardExecutionOrigin::original_subject(tx, identity, run).await
			})
			.await?
		else {
			return Ok(false);
		};
		let Ok(mut access) = Access::begin(&self.federation.store, &subject).await else {
			return Ok(false);
		};
		let policy = pool.dashboard_policy();
		// A suspension requires a separate, explicit administrator authorization.
		// Lock the Run and rebind its origin in the same transaction as resume.
		{
			use reinhardt::db::orm::Model;
			let row = Run::objects()
				.filter(Run::field_id().eq(run))
				.filter(
					Run::field_control()
						.eq(crate::apps::execution::services::states::RunControl::Paused),
				)
				.filter(Run::field_error().eq(Some("account_suspended".to_owned())))
				.select_for_update()
				.all_with_executor(access.tx.native_mut())
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.pop();
			if let Some(mut row) = row {
				let Some(epoch) = row
					.pending
					.0
					.get("recovery")
					.and_then(|v| v.get("account_suspension_epoch"))
					.and_then(|v| v.as_u64())
				else {
					return access.finish(Ok(false)).await.map_err(Into::into);
				};
				if !DashboardExecutionOrigin::rebind_suspended(
					&mut **access.tx,
					run,
					epoch,
					policy.as_ref(),
				)
				.await?
				{
					return access.finish(Ok(false)).await.map_err(Into::into);
				}
				if let Some(recovery) = row
					.pending
					.0
					.get_mut("recovery")
					.and_then(|v| v.as_object_mut())
				{
					recovery.remove("account_suspension_epoch");
				}
				row.control = crate::apps::execution::services::states::RunControl::Active;
				row.error = None;
				row.revision += 1;
				row.updated_at = Utc::now();
				Run::objects()
					.save_with_executor(access.tx.native_mut(), &row)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
				return access.finish(Ok(true)).await.map_err(Into::into);
			}
		}
		match DashboardExecutionOrigin::require_current(
			&mut **access.tx,
			run,
			policy.as_ref(),
			aidash_application::ports::accounts::Recheck::Online,
		)
		.await
		{
			Ok(()) => {}
			Err(crate::Error::Forbidden | crate::Error::Unauthorized) => {
				use reinhardt::query::{
					Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
				};
				let update = Query::update()
					.table(Alias::new("runs"))
					.value(Alias::new("error"), "external identity disabled")
					.and_where(Expr::col("id").eq(Expr::value(run)))
					.and_where(Expr::col("control").eq(Expr::value("PAUSED")))
					.and_where(Expr::col("error").eq(Expr::value("identity status unavailable")))
					.to_string(PostgresQueryBuilder);
				crate::database::native::query(&update)
					.execute(&mut **access.tx)
					.await?;
				return access.finish(Ok(false)).await.map_err(Into::into);
			}
			Err(error) => return Err(error.into()),
		}
		let scope = TenancyScope::tenant(&subject.tenant)?;
		let changed = Run::resume_identity_pause(pool, &scope, run).await;
		access.finish(changed).await.map_err(Into::into)
	}
	async fn restore(&mut self, identity: Uuid, started: DateTime<Utc>) -> Result<()> {
		self.federation
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::restore(tx, identity, started).await
			})
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
		crate::apps::identity::models::DashboardLogoutToken::revoke_sessions(
			&self.0.store.pool,
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
