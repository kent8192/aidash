use super::{Authorization, Snapshot};
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationCredential, DashboardSession,
};
use crate::{Error, Result};
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

type IdentityValidity = (String, Option<DateTime<Utc>>, Option<DateTime<Utc>>);
pub(crate) type PoolIdentityValidity =
	(String, String, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// Constructed only after authenticating a bearer token. Never deserialize this
/// from a request body, query parameter or peer-provided identity claim.
#[derive(Clone)]
pub enum Actor {
	Operator,
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

fn digest(token: &str) -> Vec<u8> {
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
		super::policy::identifier(tenant)?;
		super::policy::identifier(actor)?;
		if !(1..=2_592_000).contains(&lifetime) {
			return Err(Error::Invalid(
				"credential lifetime must be 1..2592000 seconds".into(),
			));
		}
		let token = format!(
			"aidash_subject_{}{}",
			Uuid::new_v4().simple(),
			Uuid::new_v4().simple()
		);
		let lease = self.native_connection()?;
		let credential = lease
			.handle()
			.atomic(async |tx| {
				let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant, false).await?;
				if !enabled(&snapshot, subject) {
					return Err(Error::Invalid(
						"credential subject must exist and be enabled, including its delegators"
							.into(),
					));
				}
				AuthorizationCredential::issue(tx, tenant, subject, digest(&token), lifetime, actor)
					.await
			})
			.await?;
		Ok(IssuedCredential { credential, token })
	}

	pub async fn credentials(&self, tenant: &str) -> Result<Vec<Credential>> {
		self.credentials_page(tenant, 0).await
	}
	pub async fn credentials_page(&self, tenant: &str, offset: u64) -> Result<Vec<Credential>> {
		let lease = self.native_connection()?;
		AuthorizationCredential::visible_page(&mut lease.handle(), tenant, offset).await
	}

	pub async fn revoke_credential(&self, tenant: &str, id: Uuid) -> Result<Credential> {
		let lease = self.native_connection()?;
		AuthorizationCredential::revoke(lease.handle(), tenant, id).await
	}

	pub async fn authenticate(&self, token: &str) -> Result<Actor> {
		if token.len() > 256 || !token.starts_with("aidash_subject_") {
			return Err(Error::Unauthorized);
		}
		let lease = self.native_connection()?;
		let (credential_id, tenant, subject) = lease
			.handle()
			.atomic(async |tx| AuthorizationCredential::authenticate(tx, digest(token)).await)
			.await?;
		Ok(Actor::Subject(SubjectIdentity {
			http_session: None,
			credential_id,
			tenant,
			subject,
		}))
	}
}

impl SubjectIdentity {
	/// Persist missing-Binding disablement before taking any execution lease.
	pub(crate) async fn check_binding(&self, pool: &crate::database::native::Pool) -> Result<()> {
		let col = |table: &str, name: &str| Expr::col((Alias::new(table), Alias::new(name)));
		let query = Query::select()
			.columns(
				["id", "issuer", "gcip_tenant"].map(|name| (Alias::new("i"), Alias::new(name))),
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
			.and_where(col("m", "credential_id").eq(Expr::value(self.credential_id)))
			.to_string(PostgresQueryBuilder);
		let row: Option<(Uuid, String, String)> = crate::database::native::query_as(&query)
			.columns(&["id", "issuer", "gcip_tenant"])
			.fetch_optional(pool)
			.await?;
		let Some((id, issuer, gcip_tenant)) = row else {
			return Ok(());
		};
		let allowed = match pool.dashboard_policy() {
			Some(policy) => policy
				.require_identity(
					&issuer,
					(!gcip_tenant.is_empty()).then_some(gcip_tenant.as_str()),
				)
				.is_ok(),
			None => gcip_tenant.is_empty(),
		};
		if allowed {
			return Ok(());
		}
		let lease =
			reinhardt::db::orm::connection::DatabaseConnectionLease::register(pool.connection())?;
		if crate::apps::identity::models::DashboardIdentity::disable_if_current(
			lease.handle(),
			id,
			None,
		)
		.await?
		{
			let mut db = lease.handle();
			let runs = crate::apps::identity::models::DashboardExecutionOrigin::status_waiting(
				&mut db, id,
			)
			.await?;
			crate::apps::execution::models::Run::mark_identity_disabled(&mut db, runs).await?;
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
		let valid: Option<Uuid> = {
			let query_bind_1 = self.credential_id;
			let query_bind_2 = &self.tenant;
			let query_bind_3 = &self.subject;
			crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
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
			.scalar_optional(&mut **tx)
			.await?
		};
		if valid.is_none() {
			return Err(Error::Unauthorized);
		}
		// Dashboard credentials are never exported as bearer secrets. Their
		// original mapping and external identity remain part of every durable
		// execution lease, including leases obtained after browser logout.
		let mapping: Option<(Uuid, bool, String)> = {
			let query_bind_1 = self.credential_id;
			crate::database::native::query_as(
				&Query::select()
					.columns([
						Alias::new("identity_id"),
						Alias::new("enabled"),
						Alias::new("tenant"),
					])
					.from(Alias::new("dashboard_mappings"))
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("credential_id")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
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
			let validity: Option<PoolIdentityValidity> = {
				let query_bind_1 = identity_id;
				crate::database::native::query_as(
					&Query::select()
						.columns([
							Alias::new("issuer"),
							Alias::new("gcip_tenant"),
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
				.columns(&["issuer", "gcip_tenant", "last_valid_at", "disabled_at"])
				.fetch_optional(&mut **tx)
				.await?
			};
			let (issuer, gcip_tenant, valid, disabled) = validity.ok_or(Error::Forbidden)?;
			require_dashboard_mapping(
				tx.pool().dashboard_policy().as_ref(),
				&issuer,
				&gcip_tenant,
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
	pub(crate) async fn current(
		&self,
		tx: &mut crate::database::native::Transaction,
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
				.scalar_optional(&mut **tx)
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
	aidash_domain::identity::authority::dashboard_status(
		validity,
		Utc::now(),
		crate::config::GOOGLE_OIDC_ISSUER,
	)
	.map_err(|reason| match reason {
		DashboardStatusFailure::Forbidden => Error::Forbidden,
		DashboardStatusFailure::Unavailable => Error::IdentityStatusUnavailable,
	})
}

use reinhardt::query::SimpleExpr;

/// A missing runtime policy must never admit a GCIP Mapping through a fixture or legacy path.
pub(crate) fn require_dashboard_mapping(
	policy: Option<&aidash_application::ports::authorization::dashboard::AccountPolicy>,
	issuer: &str,
	pool: &str,
	tenant: &str,
) -> Result<()> {
	match policy {
		Some(policy) => policy
			.require_mapping(issuer, (!pool.is_empty()).then_some(pool), tenant)
			.map_err(Into::into),
		None if pool.is_empty() => Ok(()),
		None => Err(Error::Forbidden),
	}
}
