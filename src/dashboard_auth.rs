//! Backend-owned OIDC login and opaque dashboard sessions.

use crate::{
	Error, Result,
	authorization::{
		Authorization,
		access::Access,
		identity::{self, Actor, SubjectIdentity},
		policy::SubjectKind,
	},
	config::OidcConfig,
	federation::Federation,
};
use axum::{
	Json, Router,
	extract::{Form, Path, Query as QueryParams, State},
	http::{HeaderMap, Method, header},
	response::{IntoResponse, Redirect, Response},
	routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use futures_util::{StreamExt, stream};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use openidconnect::{
	AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
	PkceCodeVerifier, RedirectUrl, TokenResponse,
	core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use sea_orm::sea_query::{
	Alias, Expr, JoinType, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::{collections::HashMap, sync::OnceLock, time::Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

const SESSION_COOKIE: &str = "__Host-aidash-session";
const LOGIN_COOKIE: &str = "__Host-aidash-login";
const CSRF_COOKIE: &str = "aidash-csrf";
const STATUS_FRESH_SECONDS: i64 = 300;
const STATUS_LIMIT_SECONDS: i64 = 900;
const MAX_PENDING_LOGIN_TRANSACTIONS: i64 = 10_000;
const DISCOVERY_CACHE_SECONDS: u64 = 300;
type IdentityValidity = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);
type DiscoveryCache = Mutex<HashMap<String, (Instant, CoreProviderMetadata)>>;
static DISCOVERY_CACHE: OnceLock<DiscoveryCache> = OnceLock::new();

fn table(name: &str) -> Alias {
	Alias::new(name)
}

fn digest(value: &str) -> Vec<u8> {
	Sha256::digest(value.as_bytes()).to_vec()
}

fn random_secret() -> String {
	// Two independent UUIDv4 values provide 244 random bits.
	format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn required_config(f: &Federation) -> Result<&OidcConfig> {
	f.config.oidc.as_ref().ok_or(Error::NotFound(
		"dashboard sign-in is not configured".into(),
	))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
	headers
		.get(header::COOKIE)?
		.to_str()
		.ok()?
		.split(';')
		.find_map(|item| {
			let (key, value) = item.trim().split_once('=')?;
			(key == name).then_some(value)
		})
}

fn secure_cookie(config: &OidcConfig) -> bool {
	config.public_origin.starts_with("https://")
}

fn cookie_name(base: &str, config: &OidcConfig) -> String {
	if secure_cookie(config) {
		base.to_owned()
	} else {
		base.trim_start_matches("__Host-").to_owned()
	}
}

fn set_cookie(
	response: &mut Response,
	name: &str,
	value: &str,
	config: &OidcConfig,
	http_only: bool,
	max_age: i64,
) -> Result<()> {
	let mut cookie = format!(
		"{}={}; Path=/; SameSite=Lax; Max-Age={max_age}",
		cookie_name(name, config),
		value
	);
	if secure_cookie(config) {
		cookie.push_str("; Secure");
	}
	if http_only {
		cookie.push_str("; HttpOnly");
	}
	response.headers_mut().append(
		header::SET_COOKIE,
		cookie
			.parse()
			.map_err(|_| Error::External("invalid session cookie".into()))?,
	);
	Ok(())
}

fn clear_cookie(
	response: &mut Response,
	name: &str,
	config: &OidcConfig,
	http_only: bool,
) -> Result<()> {
	set_cookie(response, name, "", config, http_only, 0)
}

fn no_store(response: &mut Response) {
	response.headers_mut().insert(
		header::CACHE_CONTROL,
		"no-store".parse().expect("static header"),
	);
	response.headers_mut().insert(
		header::REFERRER_POLICY,
		"no-referrer".parse().expect("static header"),
	);
}

fn oidc_http_client() -> Result<openidconnect::reqwest::Client> {
	openidconnect::reqwest::Client::builder()
		.redirect(openidconnect::reqwest::redirect::Policy::none())
		.timeout(std::time::Duration::from_secs(10))
		.build()
		.map_err(|_| Error::External("OIDC client unavailable".into()))
}

async fn provider_metadata(config: &OidcConfig) -> Result<CoreProviderMetadata> {
	let cache = DISCOVERY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
	let mut entries = cache.lock().await;
	if let Some((fetched_at, metadata)) = entries.get(&config.issuer)
		&& fetched_at.elapsed().as_secs() < DISCOVERY_CACHE_SECONDS
	{
		return Ok(metadata.clone());
	}
	let issuer = IssuerUrl::new(config.issuer.clone())
		.map_err(|_| Error::Invalid("invalid OIDC issuer".into()))?;
	let metadata = CoreProviderMetadata::discover_async(issuer, &oidc_http_client()?)
		.await
		.map_err(|_| Error::External("OIDC discovery unavailable".into()))?;
	entries.insert(config.issuer.clone(), (Instant::now(), metadata.clone()));
	Ok(metadata)
}

#[derive(Serialize)]
struct Configuration {
	enabled: bool,
	login_url: Option<&'static str>,
}

async fn configuration(State(f): State<Federation>) -> Json<Configuration> {
	Json(Configuration {
		enabled: f.config.oidc.is_some(),
		login_url: f.config.oidc.as_ref().map(|_| "/auth/login"),
	})
}

#[derive(Deserialize)]
struct LoginQuery {
	return_to: Option<String>,
}

fn return_path(value: Option<&str>) -> Result<&str> {
	let value = value.unwrap_or("/");
	if value.len() > 1024
		|| !value.starts_with('/')
		|| value.starts_with("//")
		|| value.contains('\\')
		|| value.contains('\r')
		|| value.contains('\n')
	{
		return Err(Error::Invalid("invalid return destination".into()));
	}
	Ok(value)
}

#[derive(FromRow)]
struct LoginTransaction {
	browser_hash: Vec<u8>,
	nonce: String,
	pkce_verifier: String,
	return_to: String,
	callback_uri: String,
	expires_at: DateTime<Utc>,
}

async fn login(
	State(f): State<Federation>,
	headers: HeaderMap,
	QueryParams(query): QueryParams<LoginQuery>,
) -> Result<Response> {
	let config = required_config(&f)?;
	let destination = return_path(query.return_to.as_deref())?;
	let browser = cookie_value(&headers, &cookie_name(LOGIN_COOKIE, config))
		.map(str::to_owned)
		.unwrap_or_else(random_secret);
	// Admission cleans up callbacks that were never completed. The expiry index
	// keeps cleanup efficient; the global cap also bounds burst storage use.
	let cleanup = Query::delete()
		.from_table(table("dashboard_login_transactions"))
		.and_where(Expr::col(table("expires_at")).lte(Expr::cust("clock_timestamp()")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&cleanup).execute(&f.store.pool).await?;
	let total_query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(table("dashboard_login_transactions"))
		.to_string(PostgresQueryBuilder);
	let total: i64 = sqlx::query_scalar(&total_query)
		.fetch_one(&f.store.pool)
		.await?;
	if total >= MAX_PENDING_LOGIN_TRANSACTIONS {
		return Err(Error::Conflict("too many pending sign-ins".into()));
	}
	let outstanding = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(table("dashboard_login_transactions"))
		.and_where(Expr::col(table("browser_hash")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let count: i64 = sqlx::query_scalar(&outstanding)
		.bind(digest(&browser))
		.fetch_one(&f.store.pool)
		.await?;
	if count >= 8 {
		return Err(Error::Conflict("too many pending sign-ins".into()));
	}
	let metadata = provider_metadata(config).await?;
	let callback = format!("{}/auth/callback", config.public_origin);
	let client = CoreClient::from_provider_metadata(
		metadata,
		ClientId::new(config.client_id.clone()),
		Some(ClientSecret::new(config.client_secret.clone())),
	)
	.set_redirect_uri(
		RedirectUrl::new(callback.clone())
			.map_err(|_| Error::Invalid("invalid OIDC callback URI".into()))?,
	);
	let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
	let (url, state, nonce) = client
		.authorize_url(
			CoreAuthenticationFlow::AuthorizationCode,
			CsrfToken::new_random,
			Nonce::new_random,
		)
		.set_pkce_challenge(challenge)
		.url();
	let query = Query::insert()
		.into_table(table("dashboard_login_transactions"))
		.columns([
			table("state_hash"),
			table("browser_hash"),
			table("nonce"),
			table("pkce_verifier"),
			table("return_to"),
			table("callback_uri"),
			table("expires_at"),
		])
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("$5"),
			Expr::cust("$6"),
			Expr::cust("$7"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(digest(state.secret()))
		.bind(digest(&browser))
		.bind(nonce.secret())
		.bind(verifier.secret())
		.bind(destination)
		.bind(callback)
		.bind(Utc::now() + Duration::minutes(5))
		.execute(&f.store.pool)
		.await?;
	let mut response = Redirect::temporary(url.as_str()).into_response();
	set_cookie(&mut response, LOGIN_COOKIE, &browser, config, true, 300)?;
	no_store(&mut response);
	Ok(response)
}

#[derive(Deserialize)]
struct CallbackQuery {
	state: String,
	code: String,
}

#[derive(FromRow)]
struct Identity {
	id: Uuid,
	subject: String,
	last_valid_at: Option<DateTime<Utc>>,
	disabled_at: Option<DateTime<Utc>>,
}

async fn known_identity(f: &Federation, issuer: &str, subject: &str) -> Result<Identity> {
	let query = Query::select()
		.columns([
			table("id"),
			table("subject"),
			table("last_valid_at"),
			table("disabled_at"),
		])
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("issuer")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("subject")).eq(Expr::cust("$2")))
		.to_string(PostgresQueryBuilder);
	sqlx::query_as(&query)
		.bind(issuer)
		.bind(subject)
		.fetch_one(&f.store.pool)
		.await
		.map_err(Into::into)
}

#[derive(Deserialize)]
struct ServiceToken {
	access_token: String,
}

#[derive(Deserialize)]
struct KeycloakUser {
	id: Option<String>,
	enabled: Option<bool>,
}

async fn keycloak_enabled(f: &Federation, config: &OidcConfig, subject: &str) -> Result<bool> {
	let token_url = format!(
		"{}/protocol/openid-connect/token",
		config.issuer.trim_end_matches('/')
	);
	let response = f
		.client
		.post(token_url)
		.form(&[
			("grant_type", "client_credentials"),
			("client_id", config.status_client_id.as_str()),
			("client_secret", config.status_client_secret.as_str()),
		])
		.timeout(std::time::Duration::from_secs(10))
		.send()
		.await
		.map_err(|_| Error::External("Keycloak status unavailable".into()))?;
	if !response.status().is_success() {
		return Err(Error::External("Keycloak status unavailable".into()));
	}
	let token: ServiceToken = response
		.json()
		.await
		.map_err(|_| Error::External("Keycloak status unavailable".into()))?;
	let mut url = reqwest::Url::parse(&config.keycloak_admin_url)
		.map_err(|_| Error::Invalid("invalid Keycloak admin URL".into()))?;
	url.path_segments_mut()
		.map_err(|_| Error::Invalid("invalid Keycloak admin URL".into()))?
		.push("users")
		.push(subject);
	let response = f
		.client
		.get(url)
		.bearer_auth(token.access_token)
		.timeout(std::time::Duration::from_secs(10))
		.send()
		.await
		.map_err(|_| Error::External("Keycloak status unavailable".into()))?;
	if response.status() == reqwest::StatusCode::NOT_FOUND {
		return Ok(false);
	}
	if !response.status().is_success() {
		return Err(Error::External("Keycloak status unavailable".into()));
	}
	let user: KeycloakUser = response
		.json()
		.await
		.map_err(|_| Error::External("Keycloak status unavailable".into()))?;
	if user.id.as_deref() != Some(subject) {
		return Err(Error::External("Keycloak status unavailable".into()));
	}
	Ok(user.enabled == Some(true))
}

async fn account_valid(
	f: &Federation,
	id: Uuid,
	subject: &str,
	last_valid_at: Option<DateTime<Utc>>,
	disabled_at: Option<DateTime<Utc>>,
) -> Result<()> {
	if disabled_at.is_some() {
		return Err(Error::Forbidden);
	}
	let config = required_config(f)?;
	let now = Utc::now();
	if last_valid_at.is_some_and(|time| time > now - Duration::seconds(STATUS_FRESH_SECONDS)) {
		return Ok(());
	}
	// The validity deadline starts when the lookup begins, not when a slow
	// upstream response finally arrives.
	let check_started_at = now;
	match keycloak_enabled(f, config, subject).await {
		Ok(true) => {
			let query = Query::update()
				.table(table("dashboard_identities"))
				.value(
					table("last_valid_at"),
					Expr::cust("greatest(coalesce(last_valid_at,$2),$2)"),
				)
				.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(table("disabled_at")).is_null())
				.to_string(PostgresQueryBuilder);
			let changed = sqlx::query(&query)
				.bind(id)
				.bind(check_started_at)
				.execute(&f.store.pool)
				.await?
				.rows_affected();
			if changed == 1 {
				Ok(())
			} else {
				Err(Error::Forbidden)
			}
		}
		Ok(false) => {
			let query = Query::update()
				.table(table("dashboard_identities"))
				.value(
					table("disabled_at"),
					Expr::cust("coalesce(disabled_at,clock_timestamp())"),
				)
				.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder);
			sqlx::query(&query).bind(id).execute(&f.store.pool).await?;
			let sessions = Query::update()
				.table(table("dashboard_sessions"))
				.value(
					table("revoked_at"),
					Expr::cust("coalesce(revoked_at,clock_timestamp())"),
				)
				.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder);
			sqlx::query(&sessions)
				.bind(id)
				.execute(&f.store.pool)
				.await?;
			mark_explicit_disable(f, id).await?;
			Err(Error::Forbidden)
		}
		Err(_)
			if last_valid_at
				.is_some_and(|time| time > now - Duration::seconds(STATUS_LIMIT_SECONDS)) =>
		{
			Ok(())
		}
		Err(_) => Err(Error::IdentityStatusUnavailable),
	}
}

async fn status_waiting_run_ids(f: &Federation, identity_id: Uuid) -> Result<Vec<Uuid>> {
	let query = Query::select()
		.column((table("o"), table("run_id")))
		.from_as(table("dashboard_execution_origins"), table("o"))
		.join_as(
			JoinType::InnerJoin,
			table("runs"),
			table("r"),
			Expr::col((table("o"), table("run_id"))).equals((table("r"), table("id"))),
		)
		.and_where(Expr::col((table("o"), table("identity_id"))).eq(Expr::cust("$1")))
		.and_where(Expr::col((table("r"), table("control"))).eq("PAUSED"))
		.and_where(Expr::col((table("r"), table("error"))).eq("identity status unavailable"))
		.to_string(PostgresQueryBuilder);
	Ok(sqlx::query_scalar(&query)
		.bind(identity_id)
		.fetch_all(&f.store.pool)
		.await?)
}

async fn mark_explicit_disable(f: &Federation, identity_id: Uuid) -> Result<()> {
	for run_id in status_waiting_run_ids(f, identity_id).await? {
		let query = Query::update()
			.table(table("runs"))
			.value(table("error"), "external identity disabled")
			.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(table("control")).eq("PAUSED"))
			.and_where(Expr::col(table("error")).eq("identity status unavailable"))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&query)
			.bind(run_id)
			.execute(&f.store.pool)
			.await?;
	}
	Ok(())
}

async fn resume_status_waiting(f: &Federation, identity_id: Uuid) -> Result<()> {
	let query = Query::select()
		.columns([table("last_valid_at"), table("disabled_at")])
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let validity: Option<IdentityValidity> = sqlx::query_as(&query)
		.bind(identity_id)
		.fetch_optional(&f.store.pool)
		.await?;
	let Some((Some(last_valid_at), None)) = validity else {
		return Ok(());
	};
	if last_valid_at <= Utc::now() - Duration::seconds(STATUS_FRESH_SECONDS) {
		return Ok(());
	}
	for run_id in status_waiting_run_ids(f, identity_id).await? {
		let run = f.store.run(run_id).await?;
		if run.control != "PAUSED" || run.error.as_deref() != Some("identity status unavailable") {
			continue;
		}
		let origin = Query::select()
			.column(table("mapping_id"))
			.from(table("dashboard_execution_origins"))
			.and_where(Expr::col(table("run_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$2")))
			.to_string(PostgresQueryBuilder);
		let mapping_id: Option<Uuid> = sqlx::query_scalar(&origin)
			.bind(run_id)
			.bind(identity_id)
			.fetch_optional(&f.store.pool)
			.await?;
		let Some(mapping_id) = mapping_id else {
			continue;
		};
		let mapping = Query::select()
			.columns([table("tenant"), table("subject"), table("credential_id")])
			.from(table("dashboard_mappings"))
			.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder);
		let mapping: Option<(String, String, Uuid)> = sqlx::query_as(&mapping)
			.bind(mapping_id)
			.fetch_optional(&f.store.pool)
			.await?;
		let Some((tenant, subject, credential_id)) = mapping else {
			continue;
		};
		let grant = Query::select()
			.column(table("credential_id"))
			.from(table("authorization_execution"))
			.and_where(Expr::col(table("run_id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder);
		let original_credential: Option<Uuid> = sqlx::query_scalar(&grant)
			.bind(run_id)
			.fetch_optional(&f.store.pool)
			.await?;
		if original_credential != Some(credential_id) {
			continue;
		}
		let identity = SubjectIdentity {
			credential_id,
			tenant,
			subject,
		};
		let mut tx = f.store.pool.begin().await?;
		if identity.lock_with_mode(&mut tx, false).await.is_err() {
			continue;
		}
		let update = Query::update()
			.table(table("runs"))
			.value(table("control"), "ACTIVE")
			.value(table("error"), Expr::cust("NULL"))
			.value(table("revision"), Expr::cust("revision+1"))
			.value(table("updated_at"), Expr::cust("clock_timestamp()"))
			.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(table("control")).eq("PAUSED"))
			.and_where(Expr::col(table("error")).eq("identity status unavailable"))
			.to_string(PostgresQueryBuilder);
		let changed = sqlx::query(&update)
			.bind(run_id)
			.execute(&mut *tx)
			.await?
			.rows_affected();
		tx.commit().await?;
		if changed == 1 {
			f.notify.notify_waiters();
		}
	}
	Ok(())
}

/// Refresh identities independently of their browser sessions so ongoing work
/// retains the same hard account-validity deadline after logout.
pub async fn refresh_active(
	f: Federation,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	if f.config.oidc.is_none() {
		return Ok(());
	}
	loop {
		// Absolute expiry bounds even revoked sessions when no new login occurs.
		let prune = Query::delete()
			.from_table(table("dashboard_sessions"))
			.and_where(Expr::col(table("expires_at")).lte(Expr::cust("clock_timestamp()")))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&prune).execute(&f.store.pool).await?;
		let query = Query::select()
			.columns([
				table("id"),
				table("subject"),
				table("last_valid_at"),
				table("disabled_at"),
			])
			.from(table("dashboard_identities"))
			.and_where(Expr::col(table("disabled_at")).is_null())
			.to_string(PostgresQueryBuilder);
		let identities: Vec<Identity> = sqlx::query_as(&query).fetch_all(&f.store.pool).await?;
		stream::iter(identities.into_iter().map(|identity| {
			let f = f.clone();
			async move {
				match account_valid(&f, identity.id, &identity.subject, identity.last_valid_at, identity.disabled_at).await {
					Ok(()) => if let Err(error) = resume_status_waiting(&f, identity.id).await {
						tracing::warn!(identity_id=%identity.id, %error, "status recovery check failed");
					},
					Err(error) if !matches!(error, Error::Forbidden | Error::IdentityStatusUnavailable) => {
						tracing::warn!(identity_id=%identity.id, "Keycloak status refresh did not establish validity");
					},
					Err(_) => {},
				}
			}
		}))
		.buffer_unordered(8)
		.for_each(|_| async {})
		.await;
		tokio::select! {
			_ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {},
			_ = stopping.changed() => if *stopping.borrow() { return Ok(()); },
		}
	}
}

async fn callback(
	State(f): State<Federation>,
	headers: HeaderMap,
	QueryParams(query): QueryParams<CallbackQuery>,
) -> Result<Response> {
	let config = required_config(&f)?;
	let browser =
		cookie_value(&headers, &cookie_name(LOGIN_COOKIE, config)).ok_or(Error::Unauthorized)?;
	let deleted = Query::delete()
		.from_table(table("dashboard_login_transactions"))
		.and_where(Expr::col(table("state_hash")).eq(Expr::cust("$1")))
		.returning_all()
		.to_string(PostgresQueryBuilder);
	let transaction: LoginTransaction = sqlx::query_as(&deleted)
		.bind(digest(&query.state))
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	if transaction.expires_at <= Utc::now()
		|| transaction.browser_hash != digest(browser)
		|| transaction.callback_uri != format!("{}/auth/callback", config.public_origin)
	{
		return Err(Error::Unauthorized);
	}
	let http_client = oidc_http_client()?;
	let metadata = provider_metadata(config).await?;
	let client = CoreClient::from_provider_metadata(
		metadata,
		ClientId::new(config.client_id.clone()),
		Some(ClientSecret::new(config.client_secret.clone())),
	)
	.set_redirect_uri(
		RedirectUrl::new(transaction.callback_uri.clone()).map_err(|_| Error::Unauthorized)?,
	);
	let token_response = client
		.exchange_code(AuthorizationCode::new(query.code))
		.map_err(|_| Error::Unauthorized)?
		.set_pkce_verifier(PkceCodeVerifier::new(transaction.pkce_verifier))
		.request_async(&http_client)
		.await
		.map_err(|_| Error::Unauthorized)?;
	let id_token = token_response.id_token().ok_or(Error::Unauthorized)?;
	let claims = id_token
		.claims(&client.id_token_verifier(), &Nonce::new(transaction.nonce))
		.map_err(|_| Error::Unauthorized)?;
	let subject = claims.subject().as_str();
	// The ID token has already passed the library's signature, issuer, audience,
	// time and nonce checks. Its optional provider sid is used only for scoped
	// back-channel session revocation, never for identity or authority.
	let id_token_text = id_token.to_string();
	let payload = id_token_text.split('.').nth(1).ok_or(Error::Unauthorized)?;
	let payload = URL_SAFE_NO_PAD
		.decode(payload)
		.map_err(|_| Error::Unauthorized)?;
	let payload: Value = serde_json::from_slice(&payload).map_err(|_| Error::Unauthorized)?;
	let provider_sid = payload
		.get("sid")
		.and_then(Value::as_str)
		.map(str::to_owned);
	if !keycloak_enabled(&f, config, subject).await? {
		return Err(Error::Forbidden);
	}
	let identity_id = Uuid::new_v4();
	let insert = Query::insert()
		.into_table(table("dashboard_identities"))
		.columns([
			table("id"),
			table("issuer"),
			table("subject"),
			table("last_valid_at"),
		])
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("clock_timestamp()"),
		])
		.on_conflict(OnConflict::new().do_nothing().to_owned())
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.bind(identity_id)
		.bind(&config.issuer)
		.bind(subject)
		.execute(&f.store.pool)
		.await?;
	let identity = known_identity(&f, &config.issuer, subject).await?;
	if identity.disabled_at.is_some() {
		return Err(Error::Forbidden);
	}
	let secret = random_secret();
	let csrf = random_secret();
	let mut tx = f.store.pool.begin().await?;
	if let Some(previous) = cookie_value(&headers, &cookie_name(SESSION_COOKIE, config)) {
		let revoke = Query::update()
			.table(table("dashboard_sessions"))
			.value(
				table("revoked_at"),
				Expr::cust("coalesce(revoked_at,clock_timestamp())"),
			)
			.and_where(Expr::col(table("token_hash")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&revoke)
			.bind(digest(previous))
			.execute(&mut *tx)
			.await?;
	}
	let session_insert = Query::insert()
		.into_table(table("dashboard_sessions"))
		.columns([
			table("id"),
			table("token_hash"),
			table("csrf_hash"),
			table("identity_id"),
			table("provider_sid"),
			table("created_at"),
			table("last_activity_at"),
			table("expires_at"),
		])
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("$5"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()+make_interval(secs => $6)"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&session_insert)
		.bind(Uuid::new_v4())
		.bind(digest(&secret))
		.bind(digest(&csrf))
		.bind(identity.id)
		.bind(provider_sid)
		.bind(config.session_absolute_seconds as f64)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	let mut response = Redirect::to(&transaction.return_to).into_response();
	set_cookie(
		&mut response,
		SESSION_COOKIE,
		&secret,
		config,
		true,
		config.session_absolute_seconds,
	)?;
	set_cookie(
		&mut response,
		CSRF_COOKIE,
		&csrf,
		config,
		false,
		config.session_absolute_seconds,
	)?;
	clear_cookie(&mut response, LOGIN_COOKIE, config, true)?;
	no_store(&mut response);
	Ok(response)
}

#[derive(FromRow)]
pub struct BrowserSession {
	pub id: Uuid,
	pub identity_id: Uuid,
	pub csrf_hash: Vec<u8>,
	pub last_activity_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub revoked_at: Option<DateTime<Utc>>,
}

async fn session_record_from_headers(
	f: &Federation,
	headers: &HeaderMap,
) -> Result<BrowserSession> {
	let config = required_config(f)?;
	let secret =
		cookie_value(headers, &cookie_name(SESSION_COOKIE, config)).ok_or(Error::Unauthorized)?;
	let query = Query::select()
		.columns([
			table("id"),
			table("identity_id"),
			table("csrf_hash"),
			table("last_activity_at"),
			table("expires_at"),
			table("revoked_at"),
		])
		.from(table("dashboard_sessions"))
		.and_where(Expr::col(table("token_hash")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let session: BrowserSession = sqlx::query_as(&query)
		.bind(digest(secret))
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	if session.revoked_at.is_some()
		|| session.expires_at <= Utc::now()
		|| session.last_activity_at <= Utc::now() - Duration::seconds(config.session_idle_seconds)
	{
		return Err(Error::Unauthorized);
	}
	Ok(session)
}

pub async fn session_from_headers(f: &Federation, headers: &HeaderMap) -> Result<BrowserSession> {
	let session = session_record_from_headers(f, headers).await?;
	let identity_query = Query::select()
		.columns([
			table("id"),
			table("subject"),
			table("last_valid_at"),
			table("disabled_at"),
		])
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let identity: Identity = sqlx::query_as(&identity_query)
		.bind(session.identity_id)
		.fetch_one(&f.store.pool)
		.await?;
	account_valid(
		f,
		identity.id,
		&identity.subject,
		identity.last_valid_at,
		identity.disabled_at,
	)
	.await?;
	Ok(session)
}

pub fn csrf_allowed(
	config: &OidcConfig,
	headers: &HeaderMap,
	session: &BrowserSession,
	method: &Method,
) -> bool {
	if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
		return true;
	}
	let origin = headers
		.get(header::ORIGIN)
		.and_then(|value| value.to_str().ok());
	let csrf = headers
		.get("x-aidash-csrf")
		.and_then(|value| value.to_str().ok());
	origin == Some(config.public_origin.as_str())
		&& csrf.is_some_and(|value| digest(value) == session.csrf_hash)
}

#[derive(Clone)]
pub struct BrowserOrigin {
	pub identity_id: Uuid,
	pub mapping_id: Option<Uuid>,
}

#[derive(FromRow)]
struct AuthorizedMapping {
	identity_id: Uuid,
	credential_id: Uuid,
	tenant: String,
	subject: String,
	enabled: bool,
}

pub async fn actor_from_headers(
	f: &Federation,
	headers: &HeaderMap,
	method: &Method,
) -> Result<(Actor, BrowserOrigin)> {
	let config = required_config(f)?;
	let session = session_from_headers(f, headers).await?;
	if !csrf_allowed(config, headers, &session, method) {
		return Err(Error::Forbidden);
	}
	let selector = headers
		.get("x-aidash-context")
		.and_then(|value| value.to_str().ok())
		.ok_or(Error::Forbidden)?;
	if selector == "operator" {
		let query = Query::select()
			.column(table("identity_id"))
			.from(table("dashboard_operator_grants"))
			.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(table("enabled")).eq(true))
			.to_string(PostgresQueryBuilder);
		let grant: Option<Uuid> = sqlx::query_scalar(&query)
			.bind(session.identity_id)
			.fetch_optional(&f.store.pool)
			.await?;
		if grant.is_none() {
			return Err(Error::Forbidden);
		}
		return Ok((
			Actor::Operator,
			BrowserOrigin {
				identity_id: session.identity_id,
				mapping_id: None,
			},
		));
	}
	let id = selector
		.strip_prefix("mapping:")
		.ok_or(Error::Forbidden)?
		.parse::<Uuid>()
		.map_err(|_| Error::Forbidden)?;
	let query = Query::select()
		.columns([
			table("identity_id"),
			table("credential_id"),
			table("tenant"),
			table("subject"),
			table("enabled"),
		])
		.from(table("dashboard_mappings"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let mapping: AuthorizedMapping = sqlx::query_as(&query)
		.bind(id)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Forbidden)?;
	if mapping.identity_id != session.identity_id || !mapping.enabled {
		return Err(Error::Forbidden);
	}
	let identity = SubjectIdentity {
		credential_id: mapping.credential_id,
		tenant: mapping.tenant,
		subject: mapping.subject,
	};
	let _lease = Access::begin(&f.store, &identity).await?;
	Ok((
		Actor::Subject(identity),
		BrowserOrigin {
			identity_id: session.identity_id,
			mapping_id: Some(id),
		},
	))
}

#[derive(Serialize)]
pub struct MappingView {
	id: Uuid,
	tenant: String,
	subject: String,
}

#[derive(FromRow)]
struct MappingRow {
	id: Uuid,
	tenant: String,
	subject: String,
}

#[derive(Serialize)]
struct SessionView {
	id: Uuid,
	operator: bool,
	mappings: Vec<MappingView>,
}

async fn session_info(State(f): State<Federation>, headers: HeaderMap) -> Result<Response> {
	let session = session_from_headers(&f, &headers).await?;
	let query = Query::select()
		.columns([table("id"), table("tenant"), table("subject")])
		.from(table("dashboard_mappings"))
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("enabled")).eq(true))
		.to_string(PostgresQueryBuilder);
	let mappings: Vec<MappingRow> = sqlx::query_as(&query)
		.bind(session.identity_id)
		.fetch_all(&f.store.pool)
		.await?;
	let operator_query = Query::select()
		.column(table("identity_id"))
		.from(table("dashboard_operator_grants"))
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("enabled")).eq(true))
		.to_string(PostgresQueryBuilder);
	let operator: Option<Uuid> = sqlx::query_scalar(&operator_query)
		.bind(session.identity_id)
		.fetch_optional(&f.store.pool)
		.await?;
	let mut response = Json(SessionView {
		id: session.id,
		operator: operator.is_some(),
		mappings: mappings
			.into_iter()
			.map(|row| MappingView {
				id: row.id,
				tenant: row.tenant,
				subject: row.subject,
			})
			.collect(),
	})
	.into_response();
	no_store(&mut response);
	Ok(response)
}

#[derive(FromRow, Serialize)]
struct Registration {
	id: Uuid,
	identity_id: Uuid,
	status: String,
	created_at: DateTime<Utc>,
	expires_at: DateTime<Utc>,
	decided_at: Option<DateTime<Utc>>,
}

impl Registration {
	fn with_effective_status(mut self) -> Self {
		if self.status == "pending" && self.expires_at <= Utc::now() {
			self.status = "expired".into();
		}
		self
	}
}

fn registration_columns() -> [Alias; 6] {
	[
		"id",
		"identity_id",
		"status",
		"created_at",
		"expires_at",
		"decided_at",
	]
	.map(table)
}

async fn latest_registration(f: &Federation, identity_id: Uuid) -> Result<Option<Registration>> {
	let query = Query::select()
		.columns(registration_columns())
		.from(table("dashboard_registration_requests"))
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.order_by(table("created_at"), Order::Desc)
		.limit(1)
		.to_string(PostgresQueryBuilder);
	Ok(sqlx::query_as(&query)
		.bind(identity_id)
		.fetch_optional(&f.store.pool)
		.await?)
}

async fn registration_status(State(f): State<Federation>, headers: HeaderMap) -> Result<Response> {
	let session = session_from_headers(&f, &headers).await?;
	let mut response = Json(
		latest_registration(&f, session.identity_id)
			.await?
			.map(Registration::with_effective_status),
	)
	.into_response();
	no_store(&mut response);
	Ok(response)
}

async fn registration_create(
	State(f): State<Federation>,
	headers: HeaderMap,
) -> Result<Json<Registration>> {
	let config = required_config(&f)?;
	let session = session_from_headers(&f, &headers).await?;
	if !csrf_allowed(config, &headers, &session, &Method::POST) {
		return Err(Error::Forbidden);
	}
	let mut tx = f.store.pool.begin().await?;
	// The identity row serializes concurrent requests without weakening the
	// seven-day expiry or resetting it on duplicate submissions.
	let identity_lock = Query::select()
		.column(table("id"))
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let locked: Option<Uuid> = sqlx::query_scalar(&identity_lock)
		.bind(session.identity_id)
		.fetch_optional(&mut *tx)
		.await?;
	if locked.is_none() {
		return Err(Error::Unauthorized);
	}
	let expire = Query::update()
		.table(table("dashboard_registration_requests"))
		.value(table("status"), "expired")
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("status")).eq("pending"))
		.and_where(Expr::col(table("expires_at")).lte(Expr::cust("clock_timestamp()")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&expire)
		.bind(session.identity_id)
		.execute(&mut *tx)
		.await?;
	let latest = Query::select()
		.columns(registration_columns())
		.from(table("dashboard_registration_requests"))
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.order_by(table("created_at"), Order::Desc)
		.limit(1)
		.to_string(PostgresQueryBuilder);
	let previous: Option<Registration> = sqlx::query_as(&latest)
		.bind(session.identity_id)
		.fetch_optional(&mut *tx)
		.await?;
	if let Some(previous) = previous {
		if previous.status == "pending" {
			tx.commit().await?;
			return Ok(Json(previous));
		}
		if previous.status == "rejected"
			&& previous
				.decided_at
				.is_some_and(|time| time > Utc::now() - Duration::hours(24))
		{
			return Err(Error::Conflict(
				"registration can be resubmitted after 24 hours".into(),
			));
		}
		if previous.status == "approved" {
			let active = Query::select()
				.expr(Expr::cust("count(*)"))
				.from(table("dashboard_mappings"))
				.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
				.and_where(Expr::col(table("enabled")).eq(true))
				.to_string(PostgresQueryBuilder);
			let count: i64 = sqlx::query_scalar(&active)
				.bind(session.identity_id)
				.fetch_one(&mut *tx)
				.await?;
			if count > 0 {
				return Err(Error::Conflict("registration was already approved".into()));
			}
		}
	}
	let insert = Query::insert()
		.into_table(table("dashboard_registration_requests"))
		.columns([
			table("id"),
			table("identity_id"),
			table("status"),
			table("created_at"),
			table("expires_at"),
		])
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::value("pending"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()+interval '7 days'"),
		])
		.returning(Query::returning().columns(registration_columns()))
		.to_string(PostgresQueryBuilder);
	let registration = sqlx::query_as(&insert)
		.bind(Uuid::new_v4())
		.bind(session.identity_id)
		.fetch_one(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(Json(registration))
}

async fn admin_registrations(State(f): State<Federation>) -> Result<Json<Vec<Registration>>> {
	let query = Query::select()
		.columns(registration_columns())
		.from(table("dashboard_registration_requests"))
		.order_by(table("created_at"), Order::Desc)
		.limit(200)
		.to_string(PostgresQueryBuilder);
	let registrations: Vec<Registration> = sqlx::query_as(&query).fetch_all(&f.store.pool).await?;
	Ok(Json(
		registrations
			.into_iter()
			.map(Registration::with_effective_status)
			.collect(),
	))
}

#[derive(Serialize, FromRow)]
struct IdentityView {
	id: Uuid,
	issuer: String,
	subject: String,
	disabled_at: Option<DateTime<Utc>>,
}

async fn admin_identity(
	State(f): State<Federation>,
	Path(id): Path<Uuid>,
) -> Result<Json<IdentityView>> {
	let query = Query::select()
		.columns([
			table("id"),
			table("issuer"),
			table("subject"),
			table("disabled_at"),
		])
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let identity = sqlx::query_as(&query)
		.bind(id)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::NotFound("identity".into()))?;
	Ok(Json(identity))
}

async fn admin_restore_identity(
	State(f): State<Federation>,
	Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode> {
	let config = required_config(&f)?;
	let query = Query::select()
		.columns([
			table("id"),
			table("issuer"),
			table("subject"),
			table("disabled_at"),
		])
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let identity: IdentityView = sqlx::query_as(&query)
		.bind(id)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::NotFound("identity".into()))?;
	if identity.issuer != config.issuer || identity.disabled_at.is_none() {
		return Err(Error::Conflict(
			"identity is not disabled for the configured issuer".into(),
		));
	}
	let check_started_at = Utc::now();
	if !keycloak_enabled(&f, config, &identity.subject).await? {
		return Err(Error::Forbidden);
	}
	let update = Query::update()
		.table(table("dashboard_identities"))
		.value(table("disabled_at"), Expr::cust("NULL"))
		.value(table("last_valid_at"), Expr::cust("$2"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("disabled_at")).is_not_null())
		.to_string(PostgresQueryBuilder);
	let changed = sqlx::query(&update)
		.bind(id)
		.bind(check_started_at)
		.execute(&f.store.pool)
		.await?
		.rows_affected();
	if changed != 1 {
		return Err(Error::Conflict("identity status changed".into()));
	}
	Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn admin_identities(State(f): State<Federation>) -> Result<Json<Vec<IdentityView>>> {
	let query = Query::select()
		.columns([
			table("id"),
			table("issuer"),
			table("subject"),
			table("disabled_at"),
		])
		.from(table("dashboard_identities"))
		.order_by(table("subject"), Order::Asc)
		.limit(200)
		.to_string(PostgresQueryBuilder);
	Ok(Json(sqlx::query_as(&query).fetch_all(&f.store.pool).await?))
}

#[derive(Serialize, FromRow)]
struct AdminMapping {
	id: Uuid,
	identity_id: Uuid,
	tenant: String,
	subject: String,
	enabled: bool,
	revision: i64,
}

async fn admin_mappings(State(f): State<Federation>) -> Result<Json<Vec<AdminMapping>>> {
	let query = Query::select()
		.columns([
			table("id"),
			table("identity_id"),
			table("tenant"),
			table("subject"),
			table("enabled"),
			table("revision"),
		])
		.from(table("dashboard_mappings"))
		.order_by(table("tenant"), Order::Asc)
		.order_by(table("subject"), Order::Asc)
		.limit(200)
		.to_string(PostgresQueryBuilder);
	Ok(Json(sqlx::query_as(&query).fetch_all(&f.store.pool).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
	tenant: String,
	subject: String,
}

#[derive(Serialize)]
struct ApprovedMapping {
	id: Uuid,
	identity_id: Uuid,
	tenant: String,
	subject: String,
}

async fn admin_approve(
	State(f): State<Federation>,
	actor: Option<axum::Extension<BrowserOrigin>>,
	Path(id): Path<Uuid>,
	Json(input): Json<Approval>,
) -> Result<Json<ApprovedMapping>> {
	let mut tx = f.store.pool.begin().await?;
	let query = Query::select()
		.columns(registration_columns())
		.from(table("dashboard_registration_requests"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let registration: Registration = sqlx::query_as(&query)
		.bind(id)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::NotFound("registration".into()))?;
	if registration.status != "pending" || registration.expires_at <= Utc::now() {
		return Err(Error::Conflict("registration is no longer pending".into()));
	}
	let snapshot = Authorization::load_with_mode(&mut tx, &input.tenant, false).await?;
	if !identity::enabled(&snapshot, &input.subject)
		|| snapshot
			.bundle
			.subjects
			.get(&input.subject)
			.is_none_or(|subject| subject.kind != SubjectKind::User)
	{
		return Err(Error::Invalid(
			"approval requires an existing enabled user subject".into(),
		));
	}
	let credential_id = Uuid::new_v4();
	let credential = Query::insert()
		.into_table(table("authorization_credentials"))
		.columns([
			table("id"),
			table("tenant"),
			table("subject"),
			table("token_hash"),
			table("expires_at"),
			table("issued_by"),
		])
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("$5"),
			Expr::value("dashboard-oidc"),
		])
		.to_string(PostgresQueryBuilder);
	// No bearer value leaves this function. The credential row is a durable
	// policy lease anchor; the mapping and identity are checked at every use.
	sqlx::query(&credential)
		.bind(credential_id)
		.bind(&input.tenant)
		.bind(&input.subject)
		.bind(digest(&random_secret()))
		.bind(Utc::now() + Duration::days(3650))
		.execute(&mut *tx)
		.await?;
	let previous = Query::select()
		.columns([table("id"), table("enabled")])
		.from(table("dashboard_mappings"))
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("tenant")).eq(Expr::cust("$2")))
		.and_where(Expr::col(table("subject")).eq(Expr::cust("$3")))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let existing: Option<(Uuid, bool)> = sqlx::query_as(&previous)
		.bind(registration.identity_id)
		.bind(&input.tenant)
		.bind(&input.subject)
		.fetch_optional(&mut *tx)
		.await?;
	let mapping_id = if let Some((id, enabled)) = existing {
		if enabled {
			return Err(Error::Conflict("identity already has this mapping".into()));
		}
		let update = Query::update()
			.table(table("dashboard_mappings"))
			.value(table("credential_id"), Expr::cust("$2"))
			.value(table("enabled"), true)
			.value(table("revision"), Expr::cust("revision+1"))
			.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&update)
			.bind(id)
			.bind(credential_id)
			.execute(&mut *tx)
			.await?;
		id
	} else {
		let id = Uuid::new_v4();
		let insert = Query::insert()
			.into_table(table("dashboard_mappings"))
			.columns([
				table("id"),
				table("identity_id"),
				table("tenant"),
				table("subject"),
				table("credential_id"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("$5"),
			])
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder);
		let inserted = sqlx::query(&insert)
			.bind(id)
			.bind(registration.identity_id)
			.bind(&input.tenant)
			.bind(&input.subject)
			.bind(credential_id)
			.execute(&mut *tx)
			.await?
			.rows_affected();
		if inserted != 1 {
			return Err(Error::Conflict("identity already has this mapping".into()));
		}
		id
	};
	let decision_actor = actor.map_or_else(
		|| "operator-bearer".to_string(),
		|origin| format!("oidc:{}", origin.identity_id),
	);
	let update = Query::update()
		.table(table("dashboard_registration_requests"))
		.value(table("status"), "approved")
		.value(table("decided_at"), Expr::cust("clock_timestamp()"))
		.value(table("decision_actor"), Expr::cust("$2"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.bind(id)
		.bind(decision_actor)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(Json(ApprovedMapping {
		id: mapping_id,
		identity_id: registration.identity_id,
		tenant: input.tenant,
		subject: input.subject,
	}))
}

async fn admin_reject(
	State(f): State<Federation>,
	actor: Option<axum::Extension<BrowserOrigin>>,
	Path(id): Path<Uuid>,
) -> Result<Json<Registration>> {
	let decision_actor = actor.map_or_else(
		|| "operator-bearer".to_string(),
		|origin| format!("oidc:{}", origin.identity_id),
	);
	let query = Query::update()
		.table(table("dashboard_registration_requests"))
		.value(table("status"), "rejected")
		.value(table("decided_at"), Expr::cust("clock_timestamp()"))
		.value(table("decision_actor"), Expr::cust("$2"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("status")).eq("pending"))
		.and_where(Expr::col(table("expires_at")).gt(Expr::cust("clock_timestamp()")))
		.returning(Query::returning().columns(registration_columns()))
		.to_string(PostgresQueryBuilder);
	let registration = sqlx::query_as(&query)
		.bind(id)
		.bind(decision_actor)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Conflict("registration is no longer pending".into()))?;
	Ok(Json(registration))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorGrantInput {
	enabled: bool,
	expected_revision: i64,
}

async fn admin_operator_grant(
	State(f): State<Federation>,
	Path(id): Path<Uuid>,
	Json(input): Json<OperatorGrantInput>,
) -> Result<Json<AdminOperatorGrant>> {
	let mut tx = f.store.pool.begin().await?;
	let identity_query = Query::select()
		.column(table("disabled_at"))
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let identity: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(&identity_query)
		.bind(id)
		.fetch_optional(&mut *tx)
		.await?;
	let disabled_at = identity.ok_or(Error::NotFound("identity".into()))?;
	if input.enabled && disabled_at.is_some() {
		return Err(Error::Forbidden);
	}
	let existing = Query::select()
		.column(table("revision"))
		.from(table("dashboard_operator_grants"))
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let grant: Option<i64> = sqlx::query_scalar(&existing)
		.bind(id)
		.fetch_optional(&mut *tx)
		.await?;
	let revision = if let Some(revision) = grant {
		if input.expected_revision != revision {
			return Err(Error::Conflict("operator grant revision changed".into()));
		}
		let update = Query::update()
			.table(table("dashboard_operator_grants"))
			.value(table("enabled"), Expr::cust("$2"))
			.value(table("revision"), Expr::cust("revision+1"))
			.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(table("revision")).eq(Expr::cust("$3")))
			.to_string(PostgresQueryBuilder);
		let changed = sqlx::query(&update)
			.bind(id)
			.bind(input.enabled)
			.bind(revision)
			.execute(&mut *tx)
			.await?
			.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("operator grant revision changed".into()));
		}
		revision + 1
	} else {
		if input.expected_revision != 0 {
			return Err(Error::Conflict("operator grant revision changed".into()));
		}
		let insert = Query::insert()
			.into_table(table("dashboard_operator_grants"))
			.columns([table("identity_id"), table("enabled")])
			.values_panic([Expr::cust("$1"), Expr::cust("$2")])
			.to_string(PostgresQueryBuilder);
		sqlx::query(&insert)
			.bind(id)
			.bind(input.enabled)
			.execute(&mut *tx)
			.await?;
		1
	};
	tx.commit().await?;
	Ok(Json(AdminOperatorGrant {
		identity_id: id,
		enabled: input.enabled,
		revision,
	}))
}

#[derive(Serialize, FromRow)]
struct AdminOperatorGrant {
	identity_id: Uuid,
	enabled: bool,
	revision: i64,
}

async fn admin_operator_grants(
	State(f): State<Federation>,
) -> Result<Json<Vec<AdminOperatorGrant>>> {
	let query = Query::select()
		.columns([table("identity_id"), table("enabled"), table("revision")])
		.from(table("dashboard_operator_grants"))
		.to_string(PostgresQueryBuilder);
	Ok(Json(sqlx::query_as(&query).fetch_all(&f.store.pool).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MappingRevision {
	expected_revision: i64,
}

async fn admin_disable_mapping(
	State(f): State<Federation>,
	Path(id): Path<Uuid>,
	Json(input): Json<MappingRevision>,
) -> Result<axum::http::StatusCode> {
	let mut tx = f.store.pool.begin().await?;
	let query = Query::update()
		.table(table("dashboard_mappings"))
		.value(table("enabled"), false)
		.value(table("revision"), Expr::cust("revision+1"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.and_where(Expr::col(table("enabled")).eq(true))
		.and_where(Expr::col(table("revision")).eq(Expr::cust("$2")))
		.returning(Query::returning().column(table("credential_id")))
		.to_string(PostgresQueryBuilder);
	let credential_id: Uuid = sqlx::query_scalar(&query)
		.bind(id)
		.bind(input.expected_revision)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Conflict("mapping revision changed".into()))?;
	let revoke = Query::update()
		.table(table("authorization_credentials"))
		.value(
			table("revoked_at"),
			Expr::cust("coalesce(revoked_at,clock_timestamp())"),
		)
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&revoke)
		.bind(credential_id)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(axum::http::StatusCode::NO_CONTENT)
}

pub fn admin_routes() -> Router<Federation> {
	Router::new()
		.route("/registrations", get(admin_registrations))
		.route("/registrations/{id}/approve", post(admin_approve))
		.route("/registrations/{id}/reject", post(admin_reject))
		.route("/identities/{id}", get(admin_identity))
		.route("/identities/{id}/restore", post(admin_restore_identity))
		.route("/identities", get(admin_identities))
		.route(
			"/identities/{id}/operator-grant",
			post(admin_operator_grant),
		)
		.route("/operator-grants", get(admin_operator_grants))
		.route("/mappings", get(admin_mappings))
		.route("/mappings/{id}/disable", post(admin_disable_mapping))
}

async fn logout(State(f): State<Federation>, headers: HeaderMap) -> Result<Response> {
	let config = required_config(&f)?;
	let session = session_record_from_headers(&f, &headers).await?;
	if !csrf_allowed(config, &headers, &session, &Method::POST) {
		return Err(Error::Forbidden);
	}
	let query = Query::update()
		.table(table("dashboard_sessions"))
		.value(
			table("revoked_at"),
			Expr::cust("coalesce(revoked_at,clock_timestamp())"),
		)
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(session.id)
		.execute(&f.store.pool)
		.await?;
	let mut response = axum::http::StatusCode::NO_CONTENT.into_response();
	clear_cookie(&mut response, SESSION_COOKIE, config, true)?;
	clear_cookie(&mut response, CSRF_COOKIE, config, false)?;
	no_store(&mut response);
	Ok(response)
}

async fn activity(
	State(f): State<Federation>,
	headers: HeaderMap,
) -> Result<axum::http::StatusCode> {
	let config = required_config(&f)?;
	let session = session_from_headers(&f, &headers).await?;
	if !csrf_allowed(config, &headers, &session, &Method::POST) {
		return Err(Error::Forbidden);
	}
	let query = Query::update()
		.table(table("dashboard_sessions"))
		.value(table("last_activity_at"), Expr::cust("clock_timestamp()"))
		.and_where(Expr::col(table("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(session.id)
		.execute(&f.store.pool)
		.await?;
	Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn logout_all(State(f): State<Federation>, headers: HeaderMap) -> Result<Response> {
	let config = required_config(&f)?;
	let session = session_record_from_headers(&f, &headers).await?;
	if !csrf_allowed(config, &headers, &session, &Method::POST) {
		return Err(Error::Forbidden);
	}
	let query = Query::update()
		.table(table("dashboard_sessions"))
		.value(
			table("revoked_at"),
			Expr::cust("coalesce(revoked_at,clock_timestamp())"),
		)
		.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(session.identity_id)
		.execute(&f.store.pool)
		.await?;
	let mut response = axum::http::StatusCode::NO_CONTENT.into_response();
	clear_cookie(&mut response, SESSION_COOKIE, config, true)?;
	clear_cookie(&mut response, CSRF_COOKIE, config, false)?;
	no_store(&mut response);
	Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackchannelLogout {
	logout_token: String,
}

async fn backchannel_logout(
	State(f): State<Federation>,
	Form(body): Form<BackchannelLogout>,
) -> Result<axum::http::StatusCode> {
	let config = required_config(&f)?;
	if body.logout_token.len() > 16_384 {
		return Err(Error::Invalid("invalid logout token".into()));
	}
	let header = decode_header(&body.logout_token)
		.map_err(|_| Error::Invalid("invalid logout token".into()))?;
	if header.alg != Algorithm::RS256 {
		return Err(Error::Invalid(
			"unsupported logout signing algorithm".into(),
		));
	}
	let kid = header
		.kid
		.ok_or(Error::Invalid("logout token is missing a key ID".into()))?;
	let http_client = oidc_http_client()?;
	let metadata = CoreProviderMetadata::discover_async(
		IssuerUrl::new(config.issuer.clone())
			.map_err(|_| Error::Invalid("invalid OIDC issuer".into()))?,
		&http_client,
	)
	.await
	.map_err(|_| Error::External("OIDC discovery unavailable".into()))?;
	let response = f
		.client
		.get(metadata.jwks_uri().url().as_str())
		.timeout(std::time::Duration::from_secs(10))
		.send()
		.await
		.map_err(|_| Error::External("OIDC key set unavailable".into()))?;
	if !response.status().is_success()
		|| response
			.content_length()
			.is_some_and(|length| length > 1_048_576)
	{
		return Err(Error::External("OIDC key set unavailable".into()));
	}
	let keys: JwkSet = response
		.json()
		.await
		.map_err(|_| Error::External("OIDC key set unavailable".into()))?;
	let key = keys
		.find(&kid)
		.ok_or(Error::Invalid("unknown logout signing key".into()))?;
	if key
		.common
		.key_algorithm
		.as_ref()
		.is_some_and(|algorithm| algorithm.to_string() != "RS256")
	{
		return Err(Error::Invalid(
			"logout signing key algorithm mismatch".into(),
		));
	}
	let decoding_key = DecodingKey::from_jwk(key)
		.map_err(|_| Error::Invalid("invalid logout signing key".into()))?;
	let mut validation = Validation::new(Algorithm::RS256);
	validation.set_issuer(&[&config.issuer]);
	validation.set_audience(&[&config.client_id]);
	validation.set_required_spec_claims(&["iss", "aud", "iat", "exp"]);
	validation.leeway = 30;
	let claims = decode::<Value>(&body.logout_token, &decoding_key, &validation)
		.map_err(|_| Error::Invalid("invalid logout token".into()))?
		.claims;
	let object = claims
		.as_object()
		.ok_or(Error::Invalid("invalid logout token claims".into()))?;
	let event = object
		.get("events")
		.and_then(Value::as_object)
		.and_then(|events| events.get("http://schemas.openid.net/event/backchannel-logout"));
	if !event.is_some_and(Value::is_object) || object.contains_key("nonce") {
		return Err(Error::Invalid("invalid logout token claims".into()));
	}
	let sub = object
		.get("sub")
		.and_then(Value::as_str)
		.filter(|value| !value.is_empty());
	let sid = object
		.get("sid")
		.and_then(Value::as_str)
		.filter(|value| !value.is_empty());
	if sub.is_none() && sid.is_none() {
		return Err(Error::Invalid(
			"logout token needs a subject or session ID".into(),
		));
	}
	let jti = object
		.get("jti")
		.and_then(Value::as_str)
		.filter(|value| !value.is_empty() && value.len() <= 512)
		.ok_or(Error::Invalid("logout token needs a JTI".into()))?;
	let iat = object
		.get("iat")
		.and_then(Value::as_i64)
		.ok_or(Error::Invalid("invalid logout issued time".into()))?;
	let exp = object
		.get("exp")
		.and_then(Value::as_i64)
		.ok_or(Error::Invalid("invalid logout expiration".into()))?;
	let now = Utc::now().timestamp();
	if iat > now + 30 || iat < now - 86_400 || exp <= now - 30 || exp > now + 86_400 || exp <= iat {
		return Err(Error::Invalid("invalid logout token lifetime".into()));
	}
	let mut tx = f.store.pool.begin().await?;
	let replay = Query::insert()
		.into_table(table("dashboard_logout_tokens"))
		.columns([table("jti_hash"), table("expires_at")])
		.values_panic([Expr::cust("$1"), Expr::cust("$2")])
		.on_conflict(OnConflict::new().do_nothing().to_owned())
		.to_string(PostgresQueryBuilder);
	let inserted = sqlx::query(&replay)
		.bind(digest(&format!("{}:{jti}", config.issuer)))
		.bind(
			DateTime::<Utc>::from_timestamp(exp, 0)
				.ok_or(Error::Invalid("invalid logout expiration".into()))?,
		)
		.execute(&mut *tx)
		.await?
		.rows_affected();
	if inserted != 1 {
		return Err(Error::Invalid("replayed logout token".into()));
	}
	let mut identities = Query::select();
	identities
		.column(table("id"))
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("issuer")).eq(Expr::cust("$1")));
	if sub.is_some() {
		identities.and_where(Expr::col(table("subject")).eq(Expr::cust("$2")));
	}
	let identities = identities.to_string(PostgresQueryBuilder);
	let ids: Vec<Uuid> = if let Some(sub) = sub {
		sqlx::query_scalar(&identities)
			.bind(&config.issuer)
			.bind(sub)
			.fetch_all(&mut *tx)
			.await?
	} else {
		sqlx::query_scalar(&identities)
			.bind(&config.issuer)
			.fetch_all(&mut *tx)
			.await?
	};
	for id in ids {
		let mut update = Query::update();
		update
			.table(table("dashboard_sessions"))
			.value(
				table("revoked_at"),
				Expr::cust("coalesce(revoked_at,clock_timestamp())"),
			)
			.and_where(Expr::col(table("identity_id")).eq(Expr::cust("$1")));
		if sid.is_some() {
			update.and_where(Expr::col(table("provider_sid")).eq(Expr::cust("$2")));
		}
		let query = update.to_string(PostgresQueryBuilder);
		let mut statement = sqlx::query(&query).bind(id);
		if let Some(sid) = sid {
			statement = statement.bind(sid);
		}
		statement.execute(&mut *tx).await?;
	}
	tx.commit().await?;
	Ok(axum::http::StatusCode::OK)
}

pub fn routes() -> Router<Federation> {
	Router::new()
		.route("/config", get(configuration))
		.route("/login", get(login))
		.route("/callback", get(callback))
		.route("/session", get(session_info))
		.route("/activity", post(activity))
		.route(
			"/registration",
			get(registration_status).post(registration_create),
		)
		.route("/logout", post(logout))
		.route("/logout-all", post(logout_all))
		.route("/backchannel-logout", post(backchannel_logout))
}
