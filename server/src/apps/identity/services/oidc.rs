//! Backend-owned OIDC login and opaque dashboard sessions.
use crate::apps::execution::models::Run;
use crate::apps::identity::models::{
	DashboardExecutionOrigin, DashboardIdentity, DashboardLoginTransaction, DashboardLogoutToken,
	DashboardMapping, DashboardOperatorGrant, DashboardRegistrationRequest, DashboardSession,
};
use crate::http::validate;
use http::header;
use reinhardt::DiError;
use reinhardt::DiResult;
use reinhardt::Injectable;
use reinhardt::InjectionContext;
use reinhardt::injectable;

use crate::{
	Error, Result,
	authorization::{
		access::Access,
		identity::{self, Actor, SubjectIdentity},
	},
	config::OidcConfig,
	federation::Federation,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use futures_util::{StreamExt, stream};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use openidconnect::{
	AuthType, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
	PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, TokenResponse,
	core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::OnceLock, time::Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

const SESSION_COOKIE: &str = "__Host-aidash-session";
const LOGIN_COOKIE: &str = "__Host-aidash-login";
const CSRF_COOKIE: &str = "aidash-csrf";
const STATUS_FRESH_SECONDS: i64 = 300;
const STATUS_LIMIT_SECONDS: i64 = 900;
const DISCOVERY_CACHE_SECONDS: u64 = 300;
const JWKS_CACHE_SECONDS: u64 = 300;
const UNKNOWN_KID_REFRESH_SECONDS: u64 = 30;
type DiscoveryCache = Mutex<HashMap<String, (Instant, CoreProviderMetadata)>>;
static DISCOVERY_CACHE: OnceLock<DiscoveryCache> = OnceLock::new();
type JwksCache = Mutex<HashMap<String, (Instant, JwkSet)>>;
static JWKS_CACHE: OnceLock<JwksCache> = OnceLock::new();
static BACKCHANNEL_CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

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
	response.headers.append(
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
	response.headers.insert(
		header::CACHE_CONTROL,
		"no-store".parse().expect("static header"),
	);
	response.headers.insert(
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

async fn logout_decoding_key(
	f: &Federation,
	config: &OidcConfig,
	kid: &str,
) -> Result<DecodingKey> {
	let cache = JWKS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
	let mut entries = cache.lock().await;
	if let Some((fetched_at, keys)) = entries.get(&config.issuer) {
		let age = fetched_at.elapsed().as_secs();
		if age < JWKS_CACHE_SECONDS && keys.find(kid).is_some() {
			return decoding_key(keys, kid);
		}
		// An arbitrary kid must not force a provider request on every attempt.
		if age < UNKNOWN_KID_REFRESH_SECONDS {
			return Err(Error::Invalid("unknown logout signing key".into()));
		}
	}
	let metadata = provider_metadata(config).await?;
	let response = f
		.client
		.get(metadata.jwks_uri().url().as_str())
		.timeout(std::time::Duration::from_secs(10))
		.send()
		.await
		.map_err(|_| Error::External("OIDC key set unavailable".into()))?;
	if !response.status().is_success() {
		return Err(Error::External("OIDC key set unavailable".into()));
	}
	let mut response = response;
	let mut body = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| Error::External("OIDC key set unavailable".into()))?
	{
		if body.len() + chunk.len() > 1_048_576 {
			return Err(Error::External("OIDC key set unavailable".into()));
		}
		body.extend_from_slice(&chunk);
	}
	let keys: JwkSet = serde_json::from_slice(&body)
		.map_err(|_| Error::External("OIDC key set unavailable".into()))?;
	let result = decoding_key(&keys, kid);
	entries.insert(config.issuer.clone(), (Instant::now(), keys));
	result
}

fn decoding_key(keys: &JwkSet, kid: &str) -> Result<DecodingKey> {
	let key = keys
		.find(kid)
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
	DecodingKey::from_jwk(key).map_err(|_| Error::Invalid("invalid logout signing key".into()))
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
	issuer: &str,
	subject: &str,
	last_valid_at: Option<DateTime<Utc>>,
	disabled_at: Option<DateTime<Utc>>,
) -> Result<()> {
	if disabled_at.is_some() {
		return Err(Error::Forbidden);
	}
	let config = required_config(f)?;
	if issuer != config.issuer {
		return Err(Error::Forbidden);
	}
	if config.is_google() {
		return Ok(());
	}
	let now = Utc::now();
	if last_valid_at.is_some_and(|time| time > now - Duration::seconds(STATUS_FRESH_SECONDS)) {
		return Ok(());
	}
	// The validity deadline starts when the lookup begins, not when a slow
	// upstream response finally arrives.
	let check_started_at = now;
	match keycloak_enabled(f, config, subject).await {
		Ok(true) => {
			let lease = f.store.orm_connection()?;
			DashboardIdentity::record_valid(lease.handle(), id, check_started_at).await
		}
		Ok(false) => {
			disable_identity(f, id, Some(check_started_at)).await?;
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

async fn disable_identity(
	f: &Federation,
	id: Uuid,
	check_started_at: Option<DateTime<Utc>>,
) -> Result<()> {
	let lease = f.store.orm_connection()?;
	if DashboardIdentity::disable_if_current(lease.handle(), id, check_started_at).await? {
		mark_explicit_disable(f, id).await?;
	}
	Ok(())
}

async fn mark_explicit_disable(f: &Federation, identity_id: Uuid) -> Result<()> {
	let lease = f.store.orm_connection()?;
	let mut connection = lease.handle();
	let ids = DashboardExecutionOrigin::status_waiting(&mut connection, identity_id).await?;
	Run::mark_identity_disabled(&mut connection, ids).await
}

async fn resume_status_waiting(f: &Federation, identity_id: Uuid) -> Result<()> {
	let lease = f.store.orm_connection()?;
	let mut connection = lease.handle();
	let identity = DashboardIdentity::find(&mut connection, identity_id).await?;
	if identity.disabled_at.is_some()
		|| identity
			.last_valid_at
			.is_none_or(|last| last <= Utc::now() - Duration::seconds(STATUS_FRESH_SECONDS))
	{
		return Ok(());
	}
	for run in DashboardExecutionOrigin::status_waiting(&mut connection, identity_id).await? {
		let Some(subject) =
			DashboardExecutionOrigin::original_subject(&mut connection, identity_id, run).await?
		else {
			continue;
		};
		let Ok(access) = Access::begin(&f.store, &subject).await else {
			continue;
		};
		let changed = Run::resume_identity_pause(connection, run).await;
		if access.finish(changed).await? {
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
		let identities = async {
			let lease = f.store.orm_connection()?;
			DashboardIdentity::active(
				lease.handle(),
				f.config
					.oidc
					.as_ref()
					.expect("OIDC configured")
					.session_idle_seconds,
			)
			.await
		}
		.await;
		match identities {
			Ok(identities) => {
				stream::iter(
					identities
						.into_iter()
						.map(|identity| refresh_identity(f.clone(), identity)),
				)
				.buffer_unordered(8)
				.for_each(|_| async {})
				.await;
			}
			Err(error) => {
				tracing::warn!(%error, "OIDC refresh pass failed; retrying after interval")
			}
		}
		tokio::select! {
			_ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {},
			_ = stopping.changed() => if *stopping.borrow() { return Ok(()); },
		}
	}
}

async fn refresh_identity(f: Federation, identity: DashboardIdentity) {
	if f.config.oidc.as_ref().is_some_and(OidcConfig::is_google) {
		return;
	}
	if f.config
		.oidc
		.as_ref()
		.is_some_and(|config| identity.issuer != config.issuer)
	{
		if let Err(error) = disable_identity(&f, identity.id, None).await {
			tracing::warn!(identity_id=%identity.id, %error, "old-issuer identity could not be disabled");
		}
		return;
	}
	match account_valid(
		&f,
		identity.id,
		&identity.issuer,
		&identity.subject,
		identity.last_valid_at,
		identity.disabled_at,
	)
	.await
	{
		Ok(()) => {
			if let Err(error) = resume_status_waiting(&f, identity.id).await {
				tracing::warn!(identity_id=%identity.id, %error, "status recovery check failed");
			}
		}
		Err(error) if !matches!(error, Error::Forbidden | Error::IdentityStatusUnavailable) => {
			tracing::warn!(identity_id=%identity.id, "Keycloak status refresh did not establish validity");
		}
		Err(_) => {}
	}
}

pub type BrowserSession = DashboardSession;

async fn session_record_from_headers(
	f: &Federation,
	headers: &HeaderMap,
) -> Result<BrowserSession> {
	let config = required_config(f)?;
	let secret =
		cookie_value(headers, &cookie_name(SESSION_COOKIE, config)).ok_or(Error::Unauthorized)?;
	let lease = f.store.orm_connection()?;
	let session = DashboardSession::from_token(&mut lease.handle(), digest(secret)).await?;
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
	let lease = f.store.orm_connection()?;
	let identity = DashboardIdentity::find(&mut lease.handle(), session.identity_id()).await?;
	account_valid(
		f,
		identity.id,
		&identity.issuer,
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
	pub(crate) session: identity::HttpSession,
}

pub async fn actor_from_headers(
	f: &Federation,
	headers: &HeaderMap,
	method: &Method,
) -> Result<(Actor, BrowserOrigin)> {
	let config = required_config(f)?;
	let session = session_from_headers(f, headers).await?;
	let http_session = identity::HttpSession {
		id: session.id,
		identity_id: session.identity_id(),
		idle_seconds: config.session_idle_seconds,
	};
	if !csrf_allowed(config, headers, &session, method) {
		return Err(Error::Forbidden);
	}
	let selector = headers
		.get("x-aidash-context")
		.and_then(|value| value.to_str().ok())
		.ok_or(Error::Forbidden)?;
	let lease = f.store.orm_connection()?;
	let mut connection = lease.handle();
	if selector == "operator" {
		if !DashboardOperatorGrant::enabled_for_identity(&mut connection, session.identity_id())
			.await?
		{
			return Err(Error::Forbidden);
		}
		return Ok((
			Actor::Operator,
			BrowserOrigin {
				identity_id: session.identity_id(),
				mapping_id: None,
				session: http_session.clone(),
			},
		));
	}
	let id = selector
		.strip_prefix("mapping:")
		.ok_or(Error::Forbidden)?
		.parse::<Uuid>()
		.map_err(|_| Error::Forbidden)?;
	let mapping = DashboardMapping::find(&mut connection, id)
		.await?
		.ok_or(Error::Forbidden)?;
	if mapping.identity_id() != session.identity_id() || !mapping.enabled {
		return Err(Error::Forbidden);
	}
	let identity = SubjectIdentity {
		http_session: Some(http_session.clone()),
		credential_id: mapping.credential_id(),
		tenant: mapping.tenant,
		subject: mapping.subject,
	};
	let _lease = Access::begin(&f.store, &identity).await?;
	Ok((
		Actor::Subject(identity),
		BrowserOrigin {
			identity_id: session.identity_id(),
			mapping_id: Some(id),
			session: http_session.clone(),
		},
	))
}

fn decision_actor(actor: Option<BrowserOrigin>) -> String {
	actor.map_or_else(
		|| "operator-bearer".to_owned(),
		|origin| format!("oidc:{}", origin.identity_id),
	)
}

impl Registration {
	fn with_effective_status(mut self) -> Self {
		if self.status == "pending" && self.expires_at <= Utc::now() {
			self.status = "expired".into();
		}
		self
	}
}

pub use crate::apps::identity::serializers::oidc::MappingView;
pub(crate) use crate::apps::identity::serializers::oidc::{
	AdminIdentityPage, AdminMapping, AdminMappingPage, AdminOperatorGrant, Approval,
	ApprovedMapping, BackchannelLogout, CallbackQuery, Configuration, IdentityView, KeycloakUser,
	LoginQuery, MappingRevision, OperatorGrantInput, Registration, ServiceToken, SessionView,
};

use http::{HeaderMap, Method};
use reinhardt::Response;

#[derive(Clone)]
pub struct DashboardSessions {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_sessions(#[inject] runtime: Federation) -> DashboardSessions {
	DashboardSessions { runtime }
}

impl DashboardSessions {
	pub(crate) async fn configuration(&self) -> Result<Configuration> {
		let f = self.runtime.clone();
		Ok(Configuration {
			enabled: f.config.oidc.is_some(),
			provider: if f.config.oidc.as_ref().is_none_or(OidcConfig::is_google) {
				"google"
			} else {
				"keycloak"
			},
			login_url: f.config.oidc.as_ref().map(|_| "/auth/login"),
		})
	}
	pub(crate) async fn login(&self, headers: HeaderMap, query: LoginQuery) -> Result<Response> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let destination = return_path(query.return_to.as_deref())?;
		let browser = cookie_value(&headers, &cookie_name(LOGIN_COOKIE, config))
			.map(str::to_owned)
			.unwrap_or_else(random_secret);
		let callback = format!("{}/auth/callback", config.public_origin);
		let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
		let state = CsrfToken::new_random();
		let nonce = Nonce::new_random();
		let state_hash = digest(state.secret());
		let redirect_uri = RedirectUrl::new(callback.clone())
			.map_err(|_| Error::Invalid("invalid OIDC callback URI".into()))?;
		let reservation = DashboardLoginTransaction::build()
			.state_hash(crate::apps::identity::models::byte_key::ByteKey(
				state_hash.clone(),
			))
			.browser_hash(digest(&browser))
			.nonce(nonce.secret().clone())
			.pkce_verifier(verifier.secret().clone())
			.return_to(destination)
			.callback_uri(callback)
			.expires_at(Utc::now() + Duration::minutes(5))
			.finish();
		let lease = f.store.orm_connection()?;
		reservation.reserve(lease.handle()).await?;
		let metadata = match provider_metadata(config).await {
			Ok(metadata) => metadata,
			Err(error) => {
				if let Err(cleanup_error) =
					DashboardLoginTransaction::release(&mut lease.handle(), state_hash).await
				{
					tracing::warn!(%cleanup_error, "failed to release pending OIDC login reservation");
				}
				return Err(error);
			}
		};
		let client = CoreClient::from_provider_metadata(
			metadata,
			ClientId::new(config.client_id.clone()),
			Some(ClientSecret::new(config.client_secret.clone())),
		)
		.set_redirect_uri(redirect_uri);
		let (url, _, _) = client
			.authorize_url(
				CoreAuthenticationFlow::AuthorizationCode,
				move || state.clone(),
				move || nonce.clone(),
			)
			.set_pkce_challenge(challenge)
			.url();
		let mut response = Response::temporary_redirect_preserve_method(url.as_str());
		set_cookie(&mut response, LOGIN_COOKIE, &browser, config, true, 300)?;
		no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn callback(
		&self,
		headers: HeaderMap,
		query: CallbackQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let browser = cookie_value(&headers, &cookie_name(LOGIN_COOKIE, config))
			.ok_or(Error::Unauthorized)?;
		let lease = f.store.orm_connection()?;
		let transaction =
			DashboardLoginTransaction::consume(lease.handle(), digest(&query.state)).await?;
		if transaction.expires_at <= Utc::now()
			|| transaction.browser_hash != digest(browser)
			|| transaction.callback_uri != format!("{}/auth/callback", config.public_origin)
		{
			return Err(Error::Unauthorized);
		}
		let metadata = provider_metadata(config).await?;
		let (subject, provider_sid) =
			exchange_identity(config, metadata, &transaction, query.code).await?;
		if !config.is_google() && !keycloak_enabled(&f, config, &subject).await? {
			return Err(Error::Forbidden);
		}
		let identity =
			DashboardIdentity::register(lease.handle(), &config.issuer, &subject).await?;
		if identity.disabled_at.is_some() {
			return Err(Error::Forbidden);
		}
		let secret = random_secret();
		let csrf = random_secret();
		let previous = cookie_value(&headers, &cookie_name(SESSION_COOKIE, config)).map(digest);
		DashboardSession::start(
			lease.handle(),
			identity.id,
			(digest(&secret), digest(&csrf)),
			provider_sid,
			config.session_absolute_seconds,
			previous,
		)
		.await?;
		let mut response =
			Response::new(http::StatusCode::SEE_OTHER).with_location(&transaction.return_to);
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
		no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn session_info(&self, headers: HeaderMap) -> Result<Response> {
		let f = self.runtime.clone();
		let session = session_from_headers(&f, &headers).await?;
		let lease = f.store.orm_connection()?;
		let mut connection = lease.handle();
		let mappings =
			DashboardMapping::enabled_for_identity(&mut connection, session.identity_id()).await?;
		let operator =
			DashboardOperatorGrant::enabled_for_identity(&mut connection, session.identity_id())
				.await?;
		let mut response = Response::ok().with_json(
			&(SessionView {
				id: session.id,
				operator,
				mappings: mappings
					.into_iter()
					.map(|row| MappingView {
						id: row.id,
						tenant: row.tenant,
						subject: row.subject,
					})
					.collect(),
			}),
		)?;
		no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn registration_status(&self, headers: HeaderMap) -> Result<Response> {
		let f = self.runtime.clone();
		let session = session_from_headers(&f, &headers).await?;
		let lease = f.store.orm_connection()?;
		let mut connection = lease.handle();
		let mut response = Response::ok().with_json(
			&(DashboardRegistrationRequest::latest(&mut connection, session.identity_id())
				.await?
				.map(Registration::with_effective_status)),
		)?;
		no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn registration_create(&self, headers: HeaderMap) -> Result<Registration> {
		let f = &self.runtime;
		let config = required_config(f)?;
		let session = session_from_headers(f, &headers).await?;
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let lease = f.store.orm_connection()?;
		DashboardRegistrationRequest::submit(lease.handle(), session.identity_id()).await
	}
	pub(crate) async fn admin_registrations(&self) -> Result<Vec<Registration>> {
		let lease = self.runtime.store.orm_connection()?;
		DashboardRegistrationRequest::pending(lease.handle()).await
	}
	pub(crate) async fn admin_identity(&self, id: Uuid) -> Result<IdentityView> {
		let lease = self.runtime.store.orm_connection()?;
		Ok(DashboardIdentity::find(&mut lease.handle(), id)
			.await?
			.contract())
	}
	pub(crate) async fn admin_restore_identity(&self, id: Uuid) -> Result<http::StatusCode> {
		let f = &self.runtime;
		let config = required_config(f)?;
		let lease = f.store.orm_connection()?;
		let mut connection = lease.handle();
		let identity = DashboardIdentity::find(&mut connection, id).await?;
		if identity.issuer != config.issuer || identity.disabled_at.is_none() {
			return Err(Error::Conflict(
				"identity is not disabled for the configured issuer".into(),
			));
		}
		let check_started_at = Utc::now();
		if !keycloak_enabled(f, config, &identity.subject).await? {
			return Err(Error::Forbidden);
		}
		DashboardIdentity::restore(&mut connection, id, check_started_at).await?;
		Ok(http::StatusCode::NO_CONTENT)
	}
	pub(crate) async fn admin_identities(
		&self,
		page: AdminIdentityPage,
	) -> Result<Vec<IdentityView>> {
		let lease = self.runtime.store.orm_connection()?;
		DashboardIdentity::page(&mut lease.handle(), page.offset).await
	}
	pub(crate) async fn admin_mappings(&self, page: AdminMappingPage) -> Result<Vec<AdminMapping>> {
		let lease = self.runtime.store.orm_connection()?;
		DashboardMapping::page(&mut lease.handle(), page.offset).await
	}
	pub(crate) async fn admin_approve(
		&self,
		actor: Option<BrowserOrigin>,
		id: Uuid,
		input: Approval,
	) -> Result<ApprovedMapping> {
		validate(&input)?;
		let lease = self.runtime.store.orm_connection()?;
		DashboardRegistrationRequest::approve(
			lease.handle(),
			id,
			input,
			decision_actor(actor),
			digest(&random_secret()),
		)
		.await
	}
	pub(crate) async fn admin_reject(
		&self,
		actor: Option<BrowserOrigin>,
		id: Uuid,
	) -> Result<Registration> {
		let lease = self.runtime.store.orm_connection()?;
		DashboardRegistrationRequest::reject(lease.handle(), id, decision_actor(actor)).await
	}
	pub(crate) async fn admin_operator_grant(
		&self,
		id: Uuid,
		input: OperatorGrantInput,
	) -> Result<AdminOperatorGrant> {
		validate(&input)?;
		let lease = self.runtime.store.orm_connection()?;
		DashboardOperatorGrant::change_enabled(
			lease.handle(),
			id,
			input.enabled,
			input.expected_revision,
		)
		.await
	}
	pub(crate) async fn admin_operator_grants(&self) -> Result<Vec<AdminOperatorGrant>> {
		let lease = self.runtime.store.orm_connection()?;
		DashboardOperatorGrant::list(&mut lease.handle()).await
	}
	pub(crate) async fn admin_disable_mapping(
		&self,
		id: Uuid,
		input: MappingRevision,
	) -> Result<http::StatusCode> {
		validate(&input)?;
		let lease = self.runtime.store.orm_connection()?;
		DashboardMapping::disable(lease.handle(), id, input.expected_revision).await?;
		Ok(http::StatusCode::NO_CONTENT)
	}
	pub(crate) async fn logout(&self, headers: HeaderMap) -> Result<Response> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let session = session_record_from_headers(&f, &headers).await?;
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let lease = f.store.orm_connection()?;
		DashboardSession::revoke(lease.handle(), session.id, false).await?;
		let mut response = Response::new(http::StatusCode::NO_CONTENT);
		clear_cookie(&mut response, SESSION_COOKIE, config, true)?;
		clear_cookie(&mut response, CSRF_COOKIE, config, false)?;
		no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn activity(&self, headers: HeaderMap) -> Result<http::StatusCode> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let session = session_from_headers(&f, &headers).await?;
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let lease = f.store.orm_connection()?;
		DashboardSession::record_activity(lease.handle(), session.id).await?;
		Ok(http::StatusCode::NO_CONTENT)
	}
	pub(crate) async fn logout_all(&self, headers: HeaderMap) -> Result<Response> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let session = session_record_from_headers(&f, &headers).await?;
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		let lease = f.store.orm_connection()?;
		DashboardSession::revoke(lease.handle(), session.identity_id(), true).await?;
		let mut response = Response::new(http::StatusCode::NO_CONTENT);
		clear_cookie(&mut response, SESSION_COOKIE, config, true)?;
		clear_cookie(&mut response, CSRF_COOKIE, config, false)?;
		no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn backchannel_logout(
		&self,
		body: BackchannelLogout,
	) -> Result<http::StatusCode> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let _admission = BACKCHANNEL_CAPACITY
			.try_acquire()
			.map_err(|_| Error::RateLimited)?;
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
		if kid.is_empty() || kid.len() > 256 {
			return Err(Error::Invalid("invalid logout key ID".into()));
		}
		let decoding_key = logout_decoding_key(&f, config, &kid).await?;
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
		if iat > now + 30
			|| iat < now - 86_400
			|| exp <= now - 30
			|| exp > now + 86_400
			|| exp <= iat
		{
			return Err(Error::Invalid("invalid logout token lifetime".into()));
		}
		let lease = f.store.orm_connection()?;
		DashboardLogoutToken::revoke_sessions(
			lease.handle(),
			&config.issuer,
			sub,
			sid,
			digest(&format!("{}:{jti}", config.issuer)),
			DateTime::<Utc>::from_timestamp(exp, 0)
				.ok_or(Error::Invalid("invalid logout expiration".into()))?,
		)
		.await?;
		Ok(http::StatusCode::OK)
	}
}

#[async_trait::async_trait]
impl Injectable for BrowserOrigin {
	async fn inject(ctx: &InjectionContext) -> DiResult<Self> {
		ctx.get_http_request()
			.and_then(|request| request.extensions.get::<Self>())
			.ok_or_else(|| DiError::NotFound("browser origin".into()))
	}
}

async fn exchange_identity(
	config: &OidcConfig,
	metadata: CoreProviderMetadata,
	transaction: &DashboardLoginTransaction,
	code: String,
) -> Result<(String, Option<String>)> {
	if config.is_google() {
		return exchange_google_identity(config, &metadata, transaction, &code).await;
	}
	let http_client = oidc_http_client()?;
	let client = CoreClient::from_provider_metadata(
		metadata,
		ClientId::new(config.client_id.clone()),
		Some(ClientSecret::new(config.client_secret.clone())),
	)
	.set_redirect_uri(
		RedirectUrl::new(transaction.callback_uri.clone()).map_err(|_| Error::Unauthorized)?,
	);
	let client = client.set_auth_type(AuthType::BasicAuth);
	let token_response = client
		.exchange_code(AuthorizationCode::new(code))
		.map_err(|_| Error::Unauthorized)?
		.set_pkce_verifier(PkceCodeVerifier::new(transaction.pkce_verifier.clone()))
		.request_async(&http_client)
		.await
		.map_err(|_| Error::Unauthorized)?;
	let id_token = token_response.id_token().ok_or(Error::Unauthorized)?;
	let claims = id_token
		.claims(
			&client.id_token_verifier(),
			&Nonce::new(transaction.nonce.clone()),
		)
		.map_err(|_| Error::Unauthorized)?;
	let subject = claims.subject().as_str().to_owned();
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
	Ok((subject, provider_sid))
}

async fn exchange_google_identity(
	config: &OidcConfig,
	metadata: &CoreProviderMetadata,
	transaction: &DashboardLoginTransaction,
	code: &str,
) -> Result<(String, Option<String>)> {
	// Google also issues `iss=accounts.google.com`, which the OIDC library's
	// URL-typed issuer cannot deserialize. Verify the original JWT with the
	// existing JWT library; never rewrite its signed payload.
	let response = oidc_http_client()?
		.post(
			metadata
				.token_endpoint()
				.as_ref()
				.ok_or(Error::Unauthorized)?
				.url()
				.clone(),
		)
		.form(&[
			("grant_type", "authorization_code"),
			("code", code),
			("client_id", config.client_id.as_str()),
			("client_secret", config.client_secret.as_str()),
			("redirect_uri", transaction.callback_uri.as_str()),
			("code_verifier", transaction.pkce_verifier.as_str()),
		])
		.send()
		.await
		.map_err(|_| Error::Unauthorized)?
		.error_for_status()
		.map_err(|_| Error::Unauthorized)?
		.bytes()
		.await
		.map_err(|_| Error::Unauthorized)?;
	let tokens: GoogleTokenResponse =
		serde_json::from_slice(&response).map_err(|_| Error::Unauthorized)?;
	let header = decode_header(&tokens.id_token).map_err(|_| Error::Unauthorized)?;
	let keys: JwkSet = serde_json::from_value(serde_json::to_value(metadata.jwks())?)?;
	let key = keys
		.find(header.kid.as_deref().ok_or(Error::Unauthorized)?)
		.ok_or(Error::Unauthorized)?;
	let key = DecodingKey::from_jwk(key).map_err(|_| Error::Unauthorized)?;
	let mut validation = Validation::new(Algorithm::RS256);
	validation.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);
	validation.set_audience(&[&config.client_id]);
	validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
	validation.leeway = 0;
	let claims = decode::<GoogleClaims>(&tokens.id_token, &key, &validation)
		.map_err(|_| Error::Unauthorized)?
		.claims;
	if !crate::config::same_secret(&claims.nonce, &transaction.nonce) {
		return Err(Error::Unauthorized);
	}
	Ok((claims.sub, claims.sid))
}

use crate::apps::identity::serializers::oidc::{GoogleClaims, GoogleTokenResponse};

impl BrowserOrigin {
	pub(crate) async fn require_operator(
		&self,
		tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
		lock: bool,
	) -> Result<()> {
		if self.mapping_id.is_some() {
			return Err(Error::Forbidden);
		}
		// Operator-grant writes lock the identity before the grant. Retain that
		// order, followed by the session, through the Marketplace commit.
		let mut query = Query::select();
		query
			.columns(["issuer", "last_valid_at", "disabled_at"].map(Alias::new))
			.from(Alias::new("dashboard_identities"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id")))
					.eq(Expr::cust("$1")),
			);
		if lock {
			query.lock(LockType::Share);
		}
		let status = sqlx::query_as(&query.to_string(PostgresQueryBuilder))
			.bind(self.identity_id)
			.fetch_optional(&mut **tx)
			.await?;
		identity::validate_dashboard_status(status)?;
		let mut query = Query::select();
		query
			.column(Alias::new("enabled"))
			.from(Alias::new("dashboard_operator_grants"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("identity_id")))
					.eq(Expr::cust("$1")),
			);
		if lock {
			query.lock(LockType::Share);
		}
		let enabled: Option<bool> = sqlx::query_scalar(&query.to_string(PostgresQueryBuilder))
			.bind(self.identity_id)
			.fetch_optional(&mut **tx)
			.await?;
		if enabled != Some(true) {
			return Err(Error::Forbidden);
		}
		self.session.current(tx, lock).await
	}
}

use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
