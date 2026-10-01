use super::{Authorization, Snapshot};
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use sea_orm::sea_query::{Alias, Condition, Expr, LockType, Order, PostgresQueryBuilder, Query};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
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

#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Credential {
	pub id: Uuid,
	pub tenant: String,
	pub subject: String,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub revoked_at: Option<DateTime<Utc>>,
	pub issued_by: String,
}

/// The bearer value is returned exactly once; only its SHA-256 digest is stored.
#[derive(Serialize, utoipa::ToSchema)]
pub struct IssuedCredential {
	pub credential: Credential,
	pub token: String,
}

fn digest(token: &str) -> Vec<u8> {
	Sha256::digest(token.as_bytes()).to_vec()
}

pub(crate) fn enabled(snapshot: &Snapshot, subject: &str) -> bool {
	if snapshot.bundle.validate().is_err() {
		return false;
	}
	let mut current = Some(subject);
	while let Some(id) = current {
		let Some(subject) = snapshot.bundle.subjects.get(id) else {
			return false;
		};
		if !subject.enabled {
			return false;
		}
		current = subject.delegated_by.as_deref();
	}
	true
}

impl Authorization {
	pub async fn issue_credential(
		&self,
		tenant: &str,
		subject: &str,
		lifetime: i64,
		actor: &str,
	) -> Result<IssuedCredential> {
		super::policy::identifier(actor)?;
		if !(1..=2_592_000).contains(&lifetime) {
			return Err(Error::Invalid(
				"credential lifetime must be 1..2592000 seconds".into(),
			));
		}
		let mut tx = self.pool.begin().await?;
		let snapshot = Self::load(&mut tx, tenant).await?;
		if !enabled(&snapshot, subject) {
			return Err(Error::Invalid(
				"credential subject must exist and be enabled, including its delegators".into(),
			));
		}
		// Two independently generated UUIDv4 values supply 244 random bits.
		let token = format!(
			"aidash_subject_{}{}",
			Uuid::new_v4().simple(),
			Uuid::new_v4().simple()
		);
		let credential = sqlx::query_as(
			&Query::insert()
				.into_table(Alias::new("authorization_credentials"))
				.columns([
					Alias::new("id"),
					Alias::new("tenant"),
					Alias::new("subject"),
					Alias::new("token_hash"),
					Alias::new("expires_at"),
					Alias::new("issued_by"),
				])
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::cust("$3"),
					Expr::cust("$4"),
					Expr::cust("clock_timestamp()+make_interval(secs => $5)"),
					Expr::cust("$6"),
				])
				.returning(Query::returning().columns([
					Alias::new("id"),
					Alias::new("tenant"),
					Alias::new("subject"),
					Alias::new("created_at"),
					Alias::new("expires_at"),
					Alias::new("revoked_at"),
					Alias::new("issued_by"),
				]))
				.to_string(PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(tenant)
		.bind(subject)
		.bind(digest(&token))
		.bind(lifetime as f64)
		.bind(actor)
		.fetch_one(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(IssuedCredential { credential, token })
	}

	pub async fn credentials(&self, tenant: &str) -> Result<Vec<Credential>> {
		self.credentials_page(tenant, 0).await
	}
	pub async fn credentials_page(&self, tenant: &str, offset: u64) -> Result<Vec<Credential>> {
		Ok(sqlx::query_as(
			&Query::select()
				.column(Alias::new("id"))
				.column(Alias::new("tenant"))
				.column(Alias::new("subject"))
				.column(Alias::new("created_at"))
				.column(Alias::new("expires_at"))
				.column(Alias::new("revoked_at"))
				.column(Alias::new("issued_by"))
				.from(Alias::new("authorization_credentials"))
				.cond_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.cond_where(Expr::col(Alias::new("issued_by")).ne("dashboard-oidc"))
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("id"), Order::Asc)
				.limit(200)
				.offset(offset)
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.fetch_all(&self.pool)
		.await?)
	}

	pub async fn revoke_credential(&self, tenant: &str, id: Uuid) -> Result<Credential> {
		let mut tx = self.pool.begin().await?;
		crate::transactions::authority::control(&mut tx).await?;
		let credential = sqlx::query_as(
			&Query::update()
				.table(Alias::new("authorization_credentials"))
				.value(
					Alias::new("revoked_at"),
					Expr::cust("coalesce(revoked_at,clock_timestamp())"),
				)
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
						.add(Expr::col(Alias::new("issued_by")).ne("dashboard-oidc")),
				)
				.returning(Query::returning().columns([
					Alias::new("id"),
					Alias::new("tenant"),
					Alias::new("subject"),
					Alias::new("created_at"),
					Alias::new("expires_at"),
					Alias::new("revoked_at"),
					Alias::new("issued_by"),
				]))
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.bind(id)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::NotFound("credential".into()))?;
		tx.commit().await?;
		Ok(credential)
	}

	pub async fn authenticate(&self, token: &str) -> Result<Actor> {
		if token.len() > 256 || !token.starts_with("aidash_subject_") {
			return Err(Error::Unauthorized);
		}
		let row: Option<(Uuid, String, String)> = sqlx::query_as(
			&Query::select()
				.column(Alias::new("id"))
				.column(Alias::new("tenant"))
				.column(Alias::new("subject"))
				.from(Alias::new("authorization_credentials"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("token_hash")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("revoked_at")).is_null())
						.add(Expr::cust("expires_at>clock_timestamp()")),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(digest(token))
		.fetch_optional(&self.pool)
		.await?;
		let (credential_id, tenant, subject) = row.ok_or(Error::Unauthorized)?;
		Ok(Actor::Subject(SubjectIdentity {
			http_session: None,
			credential_id,
			tenant,
			subject,
		}))
	}
}

impl SubjectIdentity {
	/// Kept through the protected transaction: a concurrent credential or policy
	/// revocation must wait for this boundary, and the next boundary reloads both.
	pub(crate) async fn lock_with_mode(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		exclusive: bool,
	) -> Result<Snapshot> {
		let snapshot = Authorization::load_with_mode(tx, &self.tenant, exclusive).await?;
		let valid: Option<Uuid> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("authorization_credentials"))
				.cond_where(
					Condition::all()
						.add(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
						.add(Expr::col(Alias::new("tenant")).eq(Expr::cust("$2")))
						.add(Expr::col(Alias::new("subject")).eq(Expr::cust("$3")))
						.add(Expr::col(Alias::new("revoked_at")).is_null())
						.add(Expr::cust("expires_at>clock_timestamp()")),
				)
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(self.credential_id)
		.bind(&self.tenant)
		.bind(&self.subject)
		.fetch_optional(&mut **tx)
		.await?;
		if valid.is_none() {
			return Err(Error::Unauthorized);
		}
		// Dashboard credentials are never exported as bearer secrets. Their
		// original mapping and external identity remain part of every durable
		// execution lease, including leases obtained after browser logout.
		let mapping: Option<(Uuid, bool)> = sqlx::query_as(
			&Query::select()
				.columns([Alias::new("identity_id"), Alias::new("enabled")])
				.from(Alias::new("dashboard_mappings"))
				.and_where(Expr::col(Alias::new("credential_id")).eq(Expr::cust("$1")))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.bind(self.credential_id)
		.fetch_optional(&mut **tx)
		.await?;
		if let Some((identity_id, mapping_enabled)) = mapping {
			if !mapping_enabled {
				return Err(Error::Forbidden);
			}
			let validity: Option<IdentityValidity> = sqlx::query_as(
				&Query::select()
					.columns([
						Alias::new("issuer"),
						Alias::new("last_valid_at"),
						Alias::new("disabled_at"),
					])
					.from(Alias::new("dashboard_identities"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.bind(identity_id)
			.fetch_optional(&mut **tx)
			.await?;
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
		tx: &mut Transaction<'_, Postgres>,
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
		tx: &mut Transaction<'_, Postgres>,
		lock: bool,
	) -> Result<()> {
		let mut query = Query::select();
		query.column(Alias::new("id")).from(Alias::new("dashboard_sessions"))
			.and_where(Expr::cust("id=$1 AND identity_id=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp() AND last_activity_at>clock_timestamp()-$3::bigint*interval '1 second'"));
		if lock {
			query.lock(LockType::Share);
		}
		let valid: Option<Uuid> = sqlx::query_scalar(&query.to_string(PostgresQueryBuilder))
			.bind(self.id)
			.bind(self.identity_id)
			.bind(self.idle_seconds)
			.fetch_optional(&mut **tx)
			.await?;
		if valid.is_none() {
			return Err(Error::Unauthorized);
		}
		Ok(())
	}
}

/// Shared provider-status rule for protected leases and non-authorizing idle checks.
pub(crate) fn validate_dashboard_status(validity: Option<IdentityValidity>) -> Result<()> {
	let Some((issuer, Some(last_valid_at), None)) = validity else {
		return Err(Error::Forbidden);
	};
	// Google has no service-account status endpoint; local revocation still applies.
	if issuer != crate::config::GOOGLE_OIDC_ISSUER
		&& last_valid_at <= Utc::now() - Duration::minutes(15)
	{
		return Err(Error::IdentityStatusUnavailable);
	}
	Ok(())
}
