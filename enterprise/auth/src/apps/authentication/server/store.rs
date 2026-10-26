// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Primary-backed ORM persistence. Lifecycle mutations use one transaction lock.
use crate::models::*;
use aidash_application::{
	Error, Result,
	ports::accounts::{Repository, Scope},
};
use aidash_domain::accounts::{Account, ExternalIdentity, FreshProof, ProductSession};
use aidash_domain::identity::dashboard::SignIn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::{
	backends::{DatabaseConnection, PostgresBackend, TransactionExecutor},
	orm::{Json, Model},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
use uuid::Uuid;

pub(crate) fn unavailable(_: impl std::fmt::Display) -> Error {
	Error::External("authentication persistence unavailable".into())
}
pub(crate) fn encode(value: &impl Serialize) -> Result<serde_json::Value> {
	Ok(serde_json::to_value(value)?)
}
pub(crate) fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T> {
	Ok(serde_json::from_value(value)?)
}

#[derive(Clone)]
pub struct Store {
	pub(crate) connection: DatabaseConnection,
	pub(crate) pool: sqlx::PgPool,
	refresh_locks: DatabaseConnection,
}
impl Store {
	pub fn from_pool(pool: sqlx::PgPool) -> Self {
		// A refresh retains its family lock while using the lifecycle/OAuth pool.
		// Keep lock holders and waiters off that pool so they cannot exhaust it.
		let refresh_pool = sqlx::postgres::PgPoolOptions::new()
			.max_connections(pool.options().get_max_connections())
			.connect_lazy_with(pool.connect_options().as_ref().clone());
		Self {
			connection: DatabaseConnection::new(Arc::new(PostgresBackend::new(pool.clone()))),
			pool,
			refresh_locks: DatabaseConnection::new(Arc::new(PostgresBackend::new(refresh_pool))),
		}
	}
	#[allow(clippy::disallowed_methods)] // This service owns an isolated schema and contains no Tenant data.
	pub(crate) async fn scope(&self) -> Result<AuthScope> {
		let mut tx = self.connection.begin().await.map_err(unavailable)?;
		// All authentication lifecycle updates and admissions share a primary lock.
		// This is deliberately coarse initially: completed stops cannot race a stale
		// replica read, first association, proof consumption or session creation.
		let (sql, args) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				Alias::new("pg_advisory_xact_lock").into_iden(),
				vec![Expr::value(7_100_338_i64).into()],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(args))
			.await
			.map_err(unavailable)?;
		let (sql, args) = Query::select()
			.expr_as(
				SimpleExpr::FunctionCall(Alias::new("pg_is_in_recovery").into_iden(), vec![]),
				Alias::new("standby"),
			)
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_one(&sql, convert_values(args))
			.await
			.map_err(unavailable)?;
		if row.data.get("standby") != Some(&reinhardt::db::backends::QueryValue::Bool(false)) {
			return Err(Error::External(
				"authentication requires the writable primary".into(),
			));
		}
		Ok(AuthScope { tx: Some(tx) })
	}
	#[allow(clippy::disallowed_methods)] // The refresh-family lock is confined to this service-owned schema.
	pub(crate) async fn refresh_lock(&self, raw: &str) -> Result<Box<dyn TransactionExecutor>> {
		use sha2::{Digest, Sha256};
		let hash = Sha256::digest(raw.as_bytes());
		let key = i32::from_be_bytes(hash[..4].try_into().expect("four hash bytes"));
		let mut tx = self.refresh_locks.begin().await.map_err(unavailable)?;
		let (sql, args) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				Alias::new("pg_advisory_xact_lock").into_iden(),
				vec![Expr::value(71_003_339_i32).into(), Expr::value(key).into()],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(args))
			.await
			.map_err(unavailable)?;
		Ok(tx)
	}
	pub async fn purge_expired(&self) -> Result<()> {
		let now = Utc::now().timestamp();
		let mut tx = self.scope().await?;
		for table in [
			"auth_flows",
			"auth_proofs",
			"auth_token_bindings",
			"auth_refresh_responses",
			"auth_sessions",
			"auth_ephemeral_profiles",
		] {
			let (sql, args) = Query::delete()
				.from_table(Alias::new(table))
				.and_where(Expr::col(Alias::new("expires_at")).lt(Expr::value(now)))
				.build(PostgresQueryBuilder);
			tx.executor()
				.execute(&sql, convert_values(args))
				.await
				.map_err(unavailable)?;
		}
		let (sql, args) = Query::delete()
			.from_table(Alias::new("auth_rate_limits"))
			.and_where(Expr::col(Alias::new("minute")).lt(Expr::value(now / 60 - 2)))
			.build(PostgresQueryBuilder);
		tx.executor()
			.execute(&sql, convert_values(args))
			.await
			.map_err(unavailable)?;
		Box::new(tx).commit().await?;
		reinhardt::auth::oauth2_server::PostgresOAuthStore::new(self.pool.clone())
			.purge_expired(now)
			.await
			.map_err(unavailable)?;
		reinhardt::auth::oidc_op::PostgresOidcStore::new(self.pool.clone())
			.purge_expired(now)
			.await
			.map_err(unavailable)?;
		Ok(())
	}
	pub async fn migrate(&self) -> Result<()> {
		let mut executor =
			reinhardt::db::migrations::DatabaseMigrationExecutor::new(self.connection.clone());
		executor
			.apply_migrations(&crate::migrations::migrations())
			.await
			.map_err(unavailable)?;
		Ok(())
	}
	/// Infrastructure migration input carries the original UUID; lookup never invents a
	/// replacement for a known legacy source and never assigns a common Account.
	pub async fn import_legacy(&self, identities: &[ExternalIdentity]) -> Result<()> {
		let mut scope = self.scope().await?;
		for identity in identities {
			if identity.first_account.is_some() || identity.linked {
				return Err(Error::Invalid(
					"legacy import must not assign Accounts".into(),
				));
			}
			if let Some(existing) = scope
				.identity_by_source(&identity.issuer, &identity.subject)
				.await?
			{
				if existing.id != identity.id {
					return Err(Error::Conflict("legacy identity UUID differs".into()));
				}
				// Replaying an export cannot overwrite a later Link, stop or
				// observation made by this authority.
				continue;
			}
			if identity
				.last_valid_at
				.is_some_and(|seen| seen > Utc::now() + chrono::Duration::seconds(30))
			{
				return Err(Error::Invalid("future legacy status observation".into()));
			}
			scope.save_identity(identity).await?;
		}
		Box::new(scope).commit().await
	}
}
#[async_trait]
impl Repository for Store {
	async fn begin(&self) -> Result<Box<dyn Scope>> {
		Ok(Box::new(self.scope().await?))
	}
}

pub(crate) struct AuthScope {
	pub(crate) tx: Option<Box<dyn TransactionExecutor>>,
}
impl AuthScope {
	pub(crate) fn executor(&mut self) -> &mut (dyn TransactionExecutor + 'static) {
		self.tx
			.as_mut()
			.expect("active authentication transaction")
			.as_mut()
	}
	pub(crate) async fn identity_by_source(
		&mut self,
		issuer: &str,
		subject: &str,
	) -> Result<Option<ExternalIdentity>> {
		IdentityRecord::objects()
			.filter(IdentityRecord::field_issuer().eq(issuer.to_owned()))
			.filter(IdentityRecord::field_subject().eq(subject.to_owned()))
			.select_for_update()
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| decode(row.document.0))
			.transpose()
	}
	pub(crate) async fn observe_source(
		&mut self,
		identity: Uuid,
		valid_since: Option<DateTime<Utc>>,
	) -> Result<()> {
		let old = SourceObservation::objects()
			.filter(SourceObservation::field_identity_id().eq(identity))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop();
		let maximum = old.as_ref().and_then(|v| v.valid_since).max(valid_since);
		let row = SourceObservation {
			identity_id: identity,
			valid_since: maximum,
		};
		if old.is_some() {
			SourceObservation::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			SourceObservation::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		if let Some(since) = maximum {
			let sessions = SessionRecord::objects()
				.filter(SessionRecord::field_identity_id().eq(identity))
				.all_with_executor(self.executor())
				.await
				.map_err(unavailable)?;
			for row in sessions {
				let mut session: ProductSession = decode(row.document.0)?;
				if session.issued_at < since && session.revoked_at.is_none() {
					session.revoked_at = Some(Utc::now());
					self.save_session(&session).await?;
				}
			}
		}
		Ok(())
	}
	pub(crate) async fn session_kind(&mut self, id: Uuid) -> Result<Option<String>> {
		Ok(SessionRecord::objects()
			.filter(SessionRecord::field_id().eq(id))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| row.session_kind))
	}
	pub(crate) async fn mark_desktop(&mut self, id: Uuid) -> Result<()> {
		let mut row = SessionRecord::objects()
			.filter(SessionRecord::field_id().eq(id))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.ok_or(Error::Unauthorized)?;
		row.session_kind = "desktop".into();
		SessionRecord::objects()
			.save_with_executor(self.executor(), &row)
			.await
			.map_err(unavailable)?;
		Ok(())
	}

	pub(crate) async fn put_proof(&mut self, proof: &FreshProof) -> Result<()> {
		ProofRecord::objects()
			.insert_with_executor(
				self.executor(),
				&ProofRecord {
					id: proof.id,
					expires_at: proof.expires_at.timestamp(),
					document: Json(encode(proof)?),
				},
			)
			.await
			.map_err(unavailable)?;
		Ok(())
	}
	/// Only the verified upstream SignIn at this proof's exact source may supply
	/// display metadata. A client assertion or another linked identity cannot.
	pub(crate) async fn put_proof_profile(
		&mut self,
		proof: &FreshProof,
		sign_in: &SignIn,
	) -> Result<()> {
		let identity = self
			.identity(proof.identity_id)
			.await?
			.ok_or(Error::Unauthorized)?;
		if identity.issuer != sign_in.issuer
			|| identity.subject != sign_in.subject
			|| proof.authenticated_at != sign_in.auth_time
			|| sign_in
				.verified_email
				.as_ref()
				.is_some_and(|value| value.is_empty() || value.len() > 320)
			|| sign_in
				.display_name
				.as_ref()
				.is_some_and(|value| value.is_empty() || value.len() > 512)
		{
			return Err(Error::Unauthorized);
		}
		EphemeralProfile::objects()
			.insert_with_executor(
				self.executor(),
				&EphemeralProfile {
					id: proof.id,
					identity_id: proof.identity_id,
					kind: "proof".into(),
					auth_time: proof.authenticated_at.timestamp(),
					expires_at: proof.expires_at.timestamp(),
					verified_email: sign_in.verified_email.clone(),
					display_name: sign_in.display_name.clone(),
				},
			)
			.await
			.map_err(unavailable)?;
		Ok(())
	}
	pub(crate) async fn copy_profile(
		&mut self,
		owner: Uuid,
		kind: &str,
		target: &ProductSession,
	) -> Result<()> {
		let Some(mut profile) = self.profile(owner).await? else {
			return Ok(());
		};
		if profile.identity_id != target.identity_id
			|| profile.auth_time != target.issued_at.timestamp()
			|| profile.kind != kind
		{
			return Err(Error::Unauthorized);
		}
		if profile.expires_at <= Utc::now().timestamp() {
			return Ok(());
		}
		profile.id = target.id;
		profile.kind = "session".into();
		profile.expires_at = target.absolute_expires_at.timestamp();
		EphemeralProfile::objects()
			.insert_with_executor(self.executor(), &profile)
			.await
			.map_err(unavailable)?;
		Ok(())
	}
	pub(crate) async fn session_profile(
		&mut self,
		session: &ProductSession,
	) -> Result<Option<EphemeralProfile>> {
		let Some(profile) = self.profile(session.id).await? else {
			return Ok(None);
		};
		if profile.identity_id != session.identity_id
			|| profile.auth_time != session.issued_at.timestamp()
			|| profile.kind != "session"
			|| profile.expires_at > session.absolute_expires_at.timestamp()
		{
			return Err(Error::Unauthorized);
		}
		Ok((profile.expires_at > Utc::now().timestamp()).then_some(profile))
	}
	async fn profile(&mut self, id: Uuid) -> Result<Option<EphemeralProfile>> {
		Ok(EphemeralProfile::objects()
			.filter(EphemeralProfile::field_id().eq(id))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop())
	}
	pub(crate) async fn flow<T: DeserializeOwned>(
		&mut self,
		id: Uuid,
		browser: &str,
	) -> Result<Option<T>> {
		FlowRecord::objects()
			.filter(FlowRecord::field_id().eq(id))
			.filter(FlowRecord::field_browser_digest().eq(browser.to_owned()))
			.select_for_update()
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| decode(row.document.0))
			.transpose()
	}
	pub(crate) async fn save_flow(
		&mut self,
		id: Uuid,
		browser: &str,
		value: &impl Serialize,
	) -> Result<()> {
		let document = encode(value)?;
		let expires_at = document
			.get("expires_at")
			.and_then(|v| v.as_str())
			.and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
			.map(|v| v.timestamp());
		let row = FlowRecord {
			id,
			browser_digest: browser.into(),
			expires_at,
			document: Json(document),
		};
		let exists = !FlowRecord::objects()
			.filter(FlowRecord::field_id().eq(id))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.is_empty();
		if exists {
			FlowRecord::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			FlowRecord::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		Ok(())
	}
	pub(crate) async fn bind(
		&mut self,
		digest: String,
		session_id: Uuid,
		kind: &str,
	) -> Result<()> {
		let session = self.session(session_id).await?.ok_or(Error::Unauthorized)?;
		let expires_at = match kind {
			"code" => Utc::now().timestamp() + 300,
			"access" => Utc::now().timestamp() + 3600,
			_ => session.absolute_expires_at.timestamp(),
		};
		let row = TokenBinding {
			digest,
			session_id,
			kind: kind.into(),
			expires_at,
		};
		if kind == "sso"
			&& !TokenBinding::objects()
				.filter(TokenBinding::field_digest().eq(row.digest.clone()))
				.all_with_executor(self.executor())
				.await
				.map_err(unavailable)?
				.is_empty()
		{
			TokenBinding::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			TokenBinding::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		Ok(())
	}
	pub(crate) async fn binding(&mut self, digest: String, kind: &str) -> Result<Option<Uuid>> {
		Ok(TokenBinding::objects()
			.filter(TokenBinding::field_digest().eq(digest))
			.filter(TokenBinding::field_kind().eq(kind.to_owned()))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| row.session_id))
	}
	pub(crate) async fn refresh_response<T: DeserializeOwned>(
		&mut self,
		digest: &str,
	) -> Result<Option<T>> {
		RefreshResponse::objects()
			.filter(RefreshResponse::field_digest().eq(digest.to_owned()))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| decode(row.document.0))
			.transpose()
	}
	pub(crate) async fn save_refresh_response(
		&mut self,
		digest: &str,
		value: &impl Serialize,
	) -> Result<()> {
		let exists = !RefreshResponse::objects()
			.filter(RefreshResponse::field_digest().eq(digest.to_owned()))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.is_empty();
		let document = encode(value)?;
		let expires_at = document
			.get("expires_at")
			.and_then(|v| v.as_i64())
			.ok_or_else(|| Error::Invalid("refresh response expiry missing".into()))?;
		let row = RefreshResponse {
			digest: digest.into(),
			expires_at,
			document: Json(document),
		};
		if exists {
			RefreshResponse::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			RefreshResponse::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		Ok(())
	}
	pub(crate) async fn rate_limit(&mut self, bucket: &str, limit: i64) -> Result<bool> {
		let minute = Utc::now().timestamp() / 60;
		let row = RateLimit::objects()
			.filter(RateLimit::field_bucket().eq(bucket.to_owned()))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop();
		let mut current = row.unwrap_or(RateLimit {
			bucket: bucket.into(),
			minute,
			attempts: 0,
		});
		let exists = current.attempts > 0;
		if current.minute != minute {
			current.minute = minute;
			current.attempts = 0;
		}
		current.attempts += 1;
		if exists {
			RateLimit::objects()
				.save_with_executor(self.executor(), &current)
				.await
				.map_err(unavailable)?;
		} else {
			RateLimit::objects()
				.insert_with_executor(self.executor(), &current)
				.await
				.map_err(unavailable)?;
		}
		Ok(current.attempts <= limit)
	}
}
#[async_trait]
impl Scope for AuthScope {
	async fn account(&mut self, id: Uuid) -> Result<Option<Account>> {
		AccountRecord::objects()
			.filter(AccountRecord::field_id().eq(id))
			.select_for_update()
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| decode(row.document.0))
			.transpose()
	}
	async fn identity(&mut self, id: Uuid) -> Result<Option<ExternalIdentity>> {
		IdentityRecord::objects()
			.filter(IdentityRecord::field_id().eq(id))
			.select_for_update()
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| decode(row.document.0))
			.transpose()
	}
	async fn session(&mut self, id: Uuid) -> Result<Option<ProductSession>> {
		SessionRecord::objects()
			.filter(SessionRecord::field_id().eq(id))
			.select_for_update()
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop()
			.map(|row| decode(row.document.0))
			.transpose()
	}
	async fn save_account(&mut self, value: &Account) -> Result<()> {
		let exists = self.account(value.id).await?.is_some();
		let row = AccountRecord {
			id: value.id,
			document: Json(encode(value)?),
		};
		if exists {
			AccountRecord::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			AccountRecord::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		Ok(())
	}
	async fn save_identity(&mut self, value: &ExternalIdentity) -> Result<()> {
		let existing = self.identity(value.id).await?;
		if let Some(old) = &existing
			&& (old.issuer != value.issuer
				|| old.subject != value.subject
				|| old.first_account.is_some() && old.first_account != value.first_account)
		{
			return Err(Error::Conflict(
				"historical identity association is immutable".into(),
			));
		}
		let row = IdentityRecord {
			id: value.id,
			issuer: value.issuer.clone(),
			subject: value.subject.clone(),
			document: Json(encode(value)?),
		};
		if existing.is_some() {
			IdentityRecord::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			IdentityRecord::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		Ok(())
	}
	async fn save_session(&mut self, value: &ProductSession) -> Result<()> {
		let exists = self.session(value.id).await?.is_some();
		let session_kind = self.session_kind(value.id).await?.unwrap_or_else(|| {
			if value.client_id == "sso" {
				"sso"
			} else {
				"browser"
			}
			.into()
		});
		let row = SessionRecord {
			session_kind,
			id: value.id,
			expires_at: value.absolute_expires_at.timestamp(),
			account_id: value.account_id,
			identity_id: value.identity_id,
			document: Json(encode(value)?),
		};
		if exists {
			SessionRecord::objects()
				.save_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		} else {
			SessionRecord::objects()
				.insert_with_executor(self.executor(), &row)
				.await
				.map_err(unavailable)?;
		}
		Ok(())
	}
	async fn consume_proof(&mut self, id: Uuid) -> Result<Option<FreshProof>> {
		let row = ProofRecord::objects()
			.filter(ProofRecord::field_id().eq(id))
			.select_for_update()
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?
			.pop();
		let (sql, args) = Query::delete()
			.from_table(Alias::new("auth_proofs"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.build(PostgresQueryBuilder);
		self.executor()
			.execute(&sql, convert_values(args))
			.await
			.map_err(unavailable)?;
		row.map(|row| decode(row.document.0)).transpose()
	}
	async fn revoke_identity_sessions(&mut self, identity: Uuid, now: DateTime<Utc>) -> Result<()> {
		let rows = SessionRecord::objects()
			.filter(SessionRecord::field_identity_id().eq(identity))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?;
		for row in rows {
			let mut value: ProductSession = decode(row.document.0)?;
			value.revoked_at = Some(now);
			self.save_session(&value).await?;
		}
		Ok(())
	}
	async fn revoke_account_sessions(&mut self, account: Uuid, now: DateTime<Utc>) -> Result<()> {
		let rows = SessionRecord::objects()
			.filter(SessionRecord::field_account_id().eq(account))
			.all_with_executor(self.executor())
			.await
			.map_err(unavailable)?;
		for row in rows {
			let mut value: ProductSession = decode(row.document.0)?;
			value.revoked_at = Some(now);
			self.save_session(&value).await?;
		}
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.tx
			.take()
			.expect("active transaction")
			.commit()
			.await
			.map_err(unavailable)?;
		Ok(())
	}
}
