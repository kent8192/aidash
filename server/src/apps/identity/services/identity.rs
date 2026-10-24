use super::{Authorization, Snapshot};
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationCredential, DashboardSession,
};
use crate::{Error, Result};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::identity::credentials;
use aidash_domain::tenancy::TenantName;
use chrono::{DateTime, Utc};
use reinhardt::DiError;
use reinhardt::DiResult;
use reinhardt::Injectable;
use reinhardt::InjectionContext;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{Alias, Condition, Expr, LockType, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use sha2::{Digest, Sha256};

use uuid::Uuid;

pub(crate) type IdentityValidity = (String, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// Constructed only after authenticating a bearer token. Never deserialize this
/// from a request body, query parameter or peer-provided identity claim.
#[derive(Clone)]
pub enum Actor {
	DeploymentAdministrator,
	Subject(SubjectIdentity),
}

#[derive(Clone)]
pub struct SubjectIdentity {
	// Request-only authority. Durable Run identities intentionally reconstruct
	// this as None: browser logout must not revoke already admitted work.
	pub(crate) http_session: Option<HttpSession>,
	pub(crate) credential_id: Uuid,
	pub(crate) tenant: String,
	pub(crate) subject: String,
}

#[derive(Clone)]
pub(crate) struct HttpSession {
	pub id: Uuid,
	pub identity_id: Uuid,
	pub idle_seconds: i64,
	pub access_expires_at: Option<DateTime<Utc>>,
}

pub(crate) fn digest(token: &str) -> Vec<u8> {
	Sha256::digest(token.as_bytes()).to_vec()
}

pub(crate) fn enabled(snapshot: &Snapshot, subject: &str) -> bool {
	aidash_domain::identity::authority::enabled(&snapshot.bundle, subject)
}

impl Authorization {
	pub async fn issue_credential(
		&self,
		tenant: &str,
		subject: &str,
		lifetime: i64,
		actor: &str,
	) -> Result<IssuedCredential> {
		let name = TenantName::new(tenant)?;
		super::policy::identifier(actor)?;
		credentials::validate_lifetime(lifetime)?;
		// The token names its Tenant, so presenting it never reads across
		// Tenants (ADR 0047).
		let token = credentials::subject_token(&name);
		let credential = self
			.pool
			.atomic(&TenancyScope::Tenant(name), async |tx| {
				let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant, false).await?;
				if !enabled(&snapshot, subject) {
					return Err(Error::Invalid(
						"credential subject must exist and be enabled, including its delegators"
							.into(),
					));
				}
				AuthorizationCredential::issue(
					tx,
					tenant,
					subject,
					digest(&token),
					lifetime,
					actor,
					None,
				)
				.await
			})
			.await?;
		Ok(IssuedCredential { credential, token })
	}

	pub async fn credentials(&self, tenant: &str) -> Result<Vec<Credential>> {
		self.credentials_page(tenant, 0).await
	}
	pub async fn credentials_page(&self, tenant: &str, offset: u64) -> Result<Vec<Credential>> {
		self.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				crate::authorization::remote::deployment_administrator::tenant(tx, tenant).await?;
				AuthorizationCredential::visible_page(tx, tenant, None, offset).await
			})
			.await
	}

	pub async fn revoke_credential(&self, tenant: &str, id: Uuid) -> Result<Credential> {
		AuthorizationCredential::revoke(&self.pool, tenant, id).await
	}

	pub async fn authenticate(&self, token: &str) -> Result<Actor> {
		let name = credentials::subject_tenant(token).ok_or(Error::Unauthorized)?;
		let (credential_id, tenant, subject) = self
			.pool
			.atomic(&TenancyScope::Tenant(name.clone()), async |tx| {
				// A deleted Tenant's credentials look like an unknown Tenant's,
				// so nobody outside learns that it was deleted (ADR 0060).
				if crate::apps::identity::repositories::tenant_deletions::is_deleted(
					tx,
					name.as_str(),
				)
				.await?
				{
					return Err(Error::Unauthorized);
				}
				AuthorizationCredential::authenticate(tx, digest(token)).await
			})
			.await?;
		// The digest covers the Tenant name, so a credential found under another
		// Tenant means the token was not issued as presented.
		if tenant != name.as_str() {
			return Err(Error::Unauthorized);
		}
		Ok(Actor::Subject(SubjectIdentity {
			http_session: None,
			credential_id,
			tenant,
			subject,
		}))
	}
}

impl Actor {
	/// The scope of work the actor does without naming a Workspace: a
	/// Subject's own Tenant, or the Deployment Administrator's rows without a Tenant.
	pub(crate) fn tenancy(&self) -> Result<TenancyScope> {
		match self {
			Self::Subject(identity) => identity.tenancy(),
			Self::DeploymentAdministrator => Ok(TenancyScope::Deployment(
				aidash_application::tenancy::DeploymentScopeReason::DeploymentAdministration,
			)),
		}
	}

	/// The Tenant whose Registry the actor reads and writes: a Subject's own.
	/// A Deployment Administrator has none (ADR 0029).
	pub(crate) fn registry_tenant(&self) -> Result<aidash_domain::tenancy::TenantName> {
		match self {
			Self::Subject(identity) => Ok(aidash_domain::tenancy::TenantName::new(
				identity.tenant.clone(),
			)?),
			Self::DeploymentAdministrator => Err(crate::Error::Forbidden),
		}
	}
}

impl SubjectIdentity {
	/// The scope of every transaction that acts for this subject.
	pub(crate) fn tenancy(&self) -> Result<TenancyScope> {
		Ok(TenancyScope::tenant(&self.tenant)?)
	}

	/// Persist missing-Binding disablement before taking any execution lease.
	pub(crate) async fn check_binding(&self, pool: &crate::database::native::Pool) -> Result<()> {
		let col = |table: &str, name: &str| Expr::col((Alias::new(table), Alias::new(name)));
		let query = Query::select()
			.columns(
				["id", "issuer"].map(|name| (Alias::new("i"), Alias::new(name))),
			)
			.from_as(Alias::new("dashboard_identities"), Alias::new("i"))
			.join(
				reinhardt::query::JoinType::InnerJoin,
				reinhardt::query::TableRef::table_alias(
					Alias::new("dashboard_mappings"),
					Alias::new("m"),
				),
				col("m", "identity_id").equals((Alias::new("i"), Alias::new("id"))),
			)
			// A Mapping's own credential, or one its member issued through it.
			.and_where(
				Condition::any()
					.add(col("m", "credential_id").eq(Expr::value(self.credential_id)))
					.add(
						col("m", "id").in_subquery(
							Query::select()
								.column(Alias::new("mapping_id"))
								.from(Alias::new("authorization_credentials"))
								.and_where(Expr::col("id").eq(Expr::value(self.credential_id)))
								.to_owned(),
						),
					),
			)
			.to_string(PostgresQueryBuilder);
		let row: Option<(Uuid, String)> = pool
			.scoped(&self.tenancy()?, async |tx| {
				crate::database::native::query_as(&query)
					.columns(&["id", "issuer"])
					.fetch_optional(&mut **tx)
					.await
			})
			.await?;
		let Some((id, issuer)) = row else {
			return Ok(());
		};
		if admits_identity(pool.dashboard_policy().as_ref(), &issuer) {
			return Ok(());
		}
		if crate::apps::identity::models::DashboardIdentity::disable_if_current(pool, id, None)
			.await?
		{
			// The identity's Runs may belong to any Tenant.
			let scope = TenancyScope::Deployment(DeploymentScopeReason::IdentityStatus);
			pool.atomic(&scope, async |tx| {
				let runs =
					crate::apps::identity::models::DashboardExecutionOrigin::status_waiting(tx, id)
						.await?;
				crate::apps::execution::models::Run::mark_identity_disabled(tx, runs).await
			})
			.await?;
		}
		Err(Error::Forbidden)
	}
	pub(crate) async fn lock_native(
		&self,
		tx: &mut dyn TransactionExecutor,
		exclusive: bool,
		policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Result<Snapshot> {
		super::policy::identifier(&self.tenant)?;
		let snapshot = AuthorizationBundle::lock_snapshot(tx, &self.tenant, exclusive).await?;
		self.lock_holder(tx, snapshot, policy).await
	}

	/// [`Self::lock_native`] for restoring a deleted Tenant: the same checks
	/// of this subject, against the policy the Tenant was deleted with.
	pub(crate) async fn lock_deleted(
		&self,
		tx: &mut dyn TransactionExecutor,
		policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Result<Snapshot> {
		super::policy::identifier(&self.tenant)?;
		let snapshot = AuthorizationBundle::lock_frozen(tx, &self.tenant, true).await?;
		self.lock_holder(tx, snapshot, policy).await
	}

	/// Locks this subject's credential and session under an already-held policy.
	async fn lock_holder(
		&self,
		tx: &mut dyn TransactionExecutor,
		snapshot: Snapshot,
		policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	) -> Result<Snapshot> {
		AuthorizationCredential::lock_valid(
			tx,
			self.credential_id,
			&self.tenant,
			&self.subject,
			policy,
		)
		.await?;
		if !enabled(&snapshot, &self.subject) {
			return Err(Error::Forbidden);
		}
		if let Some(session) = &self.http_session {
			DashboardSession::require_current(
				tx,
				session.id,
				session.identity_id,
				session.idle_seconds,
				session.access_expires_at,
			)
			.await?;
		}
		Ok(snapshot)
	}

	/// Kept through the protected transaction: a concurrent credential or policy
	/// revocation must wait for this boundary, and the next boundary reloads both.
	pub(crate) async fn lock_with_mode(
		&self,
		tx: &mut crate::database::native::Transaction,
		exclusive: bool,
	) -> Result<Snapshot> {
		let snapshot = Authorization::load_with_mode(tx, &self.tenant, exclusive).await?;
		self.lock_credential(tx).await?;
		if !enabled(&snapshot, &self.subject) {
			return Err(Error::Forbidden);
		}
		self.session_current(tx, true).await?;
		Ok(snapshot)
	}

	/// Check another saved publisher on the already-held policy transaction.
	pub(crate) async fn lock_credential(
		&self,
		tx: &mut crate::database::native::Transaction,
	) -> Result<()> {
		let valid: Option<(Uuid, Option<Uuid>)> = {
			let query_bind_1 = self.credential_id;
			let query_bind_2 = &self.tenant;
			let query_bind_3 = &self.subject;
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("id"), Alias::new("mapping_id")])
					.from(Alias::new("authorization_credentials"))
					.and_where(
						Condition::all()
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
									SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									),
								),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_2.to_owned()).into()],
									)),
							)
							.add(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
									"subject",
								)))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								)),
							)
							.add(Expr::col(Alias::new("revoked_at")).is_null())
							.add(Expr::cust("expires_at>clock_timestamp()")),
					)
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["id", "mapping_id"])
			.fetch_optional(&mut **tx)
			.await?
		};
		let Some((_, issued_through)) = valid else {
			return Err(Error::Unauthorized);
		};
		// Dashboard credentials are never exported as bearer secrets. Their
		// original mapping and external identity remain part of every durable
		// execution lease, including leases obtained after browser logout. A
		// credential a member issued for itself follows the Mapping it was
		// issued through in the same way (ADR 0063).
		let mapping: Option<(Uuid, bool, String)> = {
			let (column, query_bind_1) = match issued_through {
				Some(mapping) => ("id", mapping),
				None => ("credential_id", self.credential_id),
			};
			crate::database::native::query_as(
				&Query::select()
					.columns([
						Alias::new("identity_id"),
						Alias::new("enabled"),
						Alias::new("tenant"),
					])
					.from(Alias::new("dashboard_mappings"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(column))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["identity_id", "enabled", "tenant"])
			.fetch_optional(&mut **tx)
			.await?
		};
		if let Some((identity_id, mapping_enabled, mapping_tenant)) = mapping {
			if !mapping_enabled || mapping_tenant != self.tenant {
				return Err(Error::Forbidden);
			}
			let validity: Option<IdentityValidity> = {
				let query_bind_1 = identity_id;
				crate::database::native::query_as(
					&Query::select()
						.columns([
							Alias::new("issuer"),
							Alias::new("last_valid_at"),
							Alias::new("disabled_at"),
						])
						.from(Alias::new("dashboard_identities"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["issuer", "last_valid_at", "disabled_at"])
				.fetch_optional(&mut **tx)
				.await?
			};
			let (issuer, valid, disabled) = validity.ok_or(Error::Forbidden)?;
			require_dashboard_mapping(
				tx.pool().dashboard_policy().as_ref(),
				&issuer,
				&mapping_tenant,
			)?;
			validate_dashboard_status(Some((issuer, valid, disabled)))?;
		}
		Ok(())
	}

	/// Order logout with an HTTP boundary after credential, mapping and identity
	/// locks. Recheck clock expiry at disclosure without reacquiring a lock.
	pub(crate) async fn session_current(
		&self,
		tx: &mut crate::database::native::Transaction,
		lock: bool,
	) -> Result<()> {
		let Some(session) = &self.http_session else {
			return Ok(());
		};
		session.current(tx, lock).await
	}
}

impl HttpSession {
	pub(crate) async fn current<E: TransactionExecutor + ?Sized>(
		&self,
		tx: &mut E,
		lock: bool,
	) -> Result<()> {
		let mut query = Query::select();
		query.column(Alias::new("id")).from(Alias::new("dashboard_sessions"))
			.and_where(Expr::cust("id=$1 AND identity_id=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp() AND last_activity_at>clock_timestamp()-$3::bigint*interval '1 second'"));
		if let Some(expires) = self.access_expires_at {
			query.and_where(Expr::value(expires).gt(Expr::cust("clock_timestamp()")));
		}
		if lock {
			query.lock(LockType::Share);
		}
		let valid: Option<Uuid> =
			crate::database::native::query_scalar(&query.to_string(PostgresQueryBuilder))
				.bind(self.id)
				.bind(self.identity_id)
				.bind(self.idle_seconds)
				.scalar_optional(&mut *tx)
				.await?;
		if valid.is_none() {
			return Err(Error::Unauthorized);
		}
		Ok(())
	}
}

#[async_trait::async_trait]
impl Injectable for Actor {
	async fn inject(ctx: &InjectionContext) -> DiResult<Self> {
		ctx.get_http_request()
			.and_then(|request| request.extensions.get::<Self>())
			.ok_or_else(|| DiError::NotFound("authenticated actor".into()))
	}
}

pub use crate::apps::identity::serializers::identity::{Credential, IssuedCredential};
pub(crate) fn validate_dashboard_status(validity: Option<IdentityValidity>) -> Result<()> {
	use aidash_domain::identity::authority::DashboardStatusFailure;
	aidash_domain::identity::authority::dashboard_status(validity, Utc::now(), reports_status)
		.map_err(|reason| match reason {
			DashboardStatusFailure::Forbidden => Error::Forbidden,
			DashboardStatusFailure::Unavailable => Error::IdentityStatusUnavailable,
		})
}

use reinhardt::query::SimpleExpr;

/// Whether `issuer` reports Account Status: only the sign-in a build adds
/// reads it, for the issuers it owns.
pub(crate) fn reports_status(issuer: &str) -> bool {
	crate::bootstrap::build::current()
		.extension
		.owns_issuer(issuer)
}

/// Whether External Identities of `issuer` may act at all. Without a
/// configured sign-in, only identities of an issuer that no build's sign-in
/// owns, such as fixtures', are unrestricted.
fn admits_identity(
	policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	issuer: &str,
) -> bool {
	match policy {
		Some(policy) => policy.require_identity(issuer).is_ok(),
		None => !crate::bootstrap::build::current()
			.extension
			.owns_issuer(issuer),
	}
}

/// [`admits_identity`], and a Mapping only into the Tenant an issuer's
/// restriction allows.
pub(crate) fn require_dashboard_mapping(
	policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	issuer: &str,
	tenant: &str,
) -> Result<()> {
	match policy {
		Some(policy) => policy.require_mapping(issuer, tenant).map_err(Into::into),
		None if admits_identity(None, issuer) => Ok(()),
		None => Err(Error::Forbidden),
	}
}
