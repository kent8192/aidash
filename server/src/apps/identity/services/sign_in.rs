//! Browser sign-in through an identity provider that a build adds: its
//! sign-in page returns an ID token that the provider verifies. Community owns the login transaction, the shared authority and
//! the session; the provider owns its issuers and its token.
use super::oidc::{self, CSRF_COOKIE, LOGIN_COOKIE, SESSION_COOKIE};
use crate::apps::identity::models::{
	DashboardLoginTransaction, DashboardSession, byte_key::ByteKey,
};
pub use crate::apps::identity::serializers::oidc::{LoginDiscovery, LoginQuery};
use crate::config::SessionConfig;
use crate::{Error, Result, federation::Federation};
use aidash_application::ports::authorization::dashboard::{AccountStatus, Issuers};
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::dashboard::{Account, SignIn};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use http::{HeaderMap, StatusCode, header};
use reinhardt::Response;
use reinhardt::db::orm::Model;
use std::sync::Arc;

#[async_trait]
pub trait SignInProvider: Send + Sync {
	/// The `provider` that `GET /auth/config` reports.
	fn kind(&self) -> &'static str;
	fn session(&self) -> SessionConfig<'_>;
	fn issuers(&self) -> Arc<dyn Issuers>;
	fn status(&self) -> Arc<dyn AccountStatus>;
	/// `GET /auth/login`.
	async fn login(
		&self,
		f: &Federation,
		headers: HeaderMap,
		query: LoginQuery,
	) -> Result<Response>;
	/// `POST /auth/login`, which resolves user input to a sign-in.
	async fn discover(
		&self,
		f: &Federation,
		headers: HeaderMap,
		input: LoginDiscovery,
	) -> Result<Response>;
	/// What the sign-in page needs for the pending login `state` names.
	async fn transaction(
		&self,
		f: &Federation,
		headers: HeaderMap,
		state: String,
	) -> Result<Response>;
	/// Finish the pending login with the ID token the sign-in page obtained.
	async fn exchange(
		&self,
		f: &Federation,
		headers: HeaderMap,
		state: String,
		id_token: String,
	) -> Result<Response>;
}

/// A login that a sign-in began and has not finished.
pub struct Pending {
	/// The issuer whose External Identity this login must sign in.
	pub issuer: String,
	pub started_at: DateTime<Utc>,
}

pub use oidc::{no_store, return_path};

/// Refuse a request that did not come from the dashboard's own origin.
pub fn require_origin(headers: &HeaderMap, public_origin: &str) -> Result<()> {
	if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) != Some(public_origin) {
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// Send the browser to the sign-in page with `params`.
pub fn sign_in_page(params: &[(&str, &str)]) -> Result<Response> {
	let query = reqwest::Url::parse_with_params("http://localhost/sign-in", params)
		.map_err(|_| Error::Invalid("invalid sign-in destination".into()))?;
	let mut response = Response::new(StatusCode::SEE_OTHER)
		.with_location(&format!("/sign-in?{}", query.query().unwrap_or("")));
	no_store(&mut response);
	Ok(response)
}

/// Begin a browser-bound login of an External Identity of `issuer`, which
/// returns to `callback`, and send the browser to the sign-in page.
pub async fn begin(
	f: &Federation,
	headers: &HeaderMap,
	destination: &str,
	issuer: String,
	callback: String,
) -> Result<Response> {
	let session_config = oidc::required_config(f)?;
	let browser = oidc::cookie_value(headers, &oidc::cookie_name(LOGIN_COOKIE, session_config))
		.map(str::to_owned)
		.unwrap_or_else(oidc::random_secret);
	let state = oidc::random_secret();
	let now = Utc::now();
	let transaction = DashboardLoginTransaction::build()
		.state_hash(ByteKey(oidc::digest(&state)))
		.browser_hash(oidc::digest(&browser))
		.nonce(String::new())
		.pkce_verifier(String::new())
		.return_to(destination)
		.callback_uri(callback)
		.expires_at(now + Duration::minutes(10))
		.started_at(Some(now))
		.issuer(Some(issuer))
		.finish();
	transaction.reserve(&f.store.pool).await?;
	let mut response =
		Response::new(StatusCode::SEE_OTHER).with_location(&format!("/sign-in?state={state}"));
	oidc::set_cookie(
		&mut response,
		LOGIN_COOKIE,
		&browser,
		session_config,
		true,
		600,
	)?;
	no_store(&mut response);
	Ok(response)
}

/// The pending login `state` names, begun in this browser for `callback`.
pub async fn pending(
	f: &Federation,
	headers: &HeaderMap,
	state: &str,
	callback: &str,
) -> Result<Pending> {
	if state.is_empty() || state.len() > 256 {
		return Err(Error::Unauthorized);
	}
	let config = oidc::required_config(f)?;
	let browser = oidc::cookie_value(headers, &oidc::cookie_name(LOGIN_COOKIE, config))
		.ok_or(Error::Unauthorized)?;
	let transaction = f
		.store
		.pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			Ok(DashboardLoginTransaction::objects()
				.filter(
					DashboardLoginTransaction::field_state_hash().eq(ByteKey(oidc::digest(state))),
				)
				.all_with_db(tx)
				.await?)
		})
		.await?
		.pop()
		.ok_or(Error::Unauthorized)?;
	if transaction.expires_at <= Utc::now()
		|| transaction.browser_hash != oidc::digest(browser)
		|| transaction.callback_uri != callback
	{
		return Err(Error::Unauthorized);
	}
	match (transaction.issuer, transaction.started_at) {
		(Some(issuer), Some(started_at)) => Ok(Pending { issuer, started_at }),
		_ => Err(Error::Unauthorized),
	}
}

/// The issuer of the External Identity whose valid browser session `headers`
/// carry, so that a provider can sign the same person in again through it.
pub async fn session_issuer(f: &Federation, headers: &HeaderMap) -> Option<String> {
	let session = oidc::browser_session_from_headers(f, headers).await.ok()?;
	let identity = f
		.store
		.pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			crate::apps::identity::models::DashboardIdentity::find(tx, session.identity_id()).await
		})
		.await
		.ok()?;
	Some(identity.issuer)
}

/// Admit a verified sign-in through the shared authority, which also
/// disables an existing identity whose issuer is no longer admitted.
pub async fn admit(f: &Federation, sign_in: &SignIn) -> Result<Account> {
	Ok(crate::bootstrap::dashboard_authority(f)
		.admit_login(
			&mut crate::bootstrap::dashboard_login(f.store.pool.clone(), &f.config.node_id),
			sign_in,
		)
		.await?)
}

/// Consume the pending login `state` names and start a browser session for
/// `identity`, whose issuer must be the one the login began for.
pub async fn start_session(
	f: &Federation,
	headers: &HeaderMap,
	state: &str,
	identity: &Account,
	auth_time: DateTime<Utc>,
) -> Result<Response> {
	let session_config = oidc::required_config(f)?;
	let browser = oidc::cookie_value(headers, &oidc::cookie_name(LOGIN_COOKIE, session_config))
		.ok_or(Error::Unauthorized)?;
	let transaction = DashboardLoginTransaction::consume_bound(
		&f.store.pool,
		oidc::digest(state),
		oidc::digest(browser),
	)
	.await?;
	if transaction.issuer.as_deref() != Some(identity.issuer.as_str()) {
		return Err(Error::Unauthorized);
	}
	let secret = oidc::random_secret();
	let csrf = oidc::random_secret();
	let previous = oidc::cookie_value(headers, &oidc::cookie_name(SESSION_COOKIE, session_config))
		.map(oidc::digest);
	DashboardSession::start(
		&f.store.pool,
		identity.id,
		(oidc::digest(&secret), oidc::digest(&csrf)),
		None,
		auth_time,
		session_config.session_absolute_seconds,
		previous,
	)
	.await?;
	let mut response =
		Response::ok().with_json(&serde_json::json!({"return_to":transaction.return_to}))?;
	oidc::set_cookie(
		&mut response,
		SESSION_COOKIE,
		&secret,
		session_config,
		true,
		session_config.session_absolute_seconds,
	)?;
	oidc::set_cookie(
		&mut response,
		CSRF_COOKIE,
		&csrf,
		session_config,
		false,
		session_config.session_absolute_seconds,
	)?;
	oidc::set_cookie(&mut response, LOGIN_COOKIE, "", session_config, true, 0)?;
	no_store(&mut response);
	Ok(response)
}
