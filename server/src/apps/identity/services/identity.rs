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
	pub(crate) async fn lock_native(
		&self,
		tx: &mut dyn TransactionExecutor,
		exclusive: bool,
	) -> Result<Snapshot> {
		super::policy::identifier(&self.tenant)?;
		let snapshot = AuthorizationBundle::lock_snapshot(tx, &self.tenant, exclusive).await?;
		AuthorizationCredential::lock_valid(tx, self.credential_id, &self.tenant, &self.subject)
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
		let mapping: Option<(Uuid, bool)> = {
			let query_bind_1 = self.credential_id;
			crate::database::native::query_as(
				&Query::select()
					.columns([Alias::new("identity_id"), Alias::new("enabled")])
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
			.columns(&["identity_id", "enabled"])
			.fetch_optional(&mut **tx)
			.await?
		};
		if let Some((identity_id, mapping_enabled)) = mapping {
			if !mapping_enabled {
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
			validate_dashboard_status(validity)?;
		}
		if !enabled(&snapshot, &self.subject) {
			return Err(Error::Forbidden);
		}
		self.session_current(tx, true).await?;
		Ok(snapshot)
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
