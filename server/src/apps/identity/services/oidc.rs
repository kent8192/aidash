//! Backend-owned OIDC login and opaque dashboard sessions.
use crate::apps::identity::models::{
	DashboardIdentity, DashboardLoginTransaction, DashboardMapping, DashboardOperatorGrant,
	DashboardRegistrationRequest, DashboardSession,
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

use chrono::{DateTime, Duration, Utc};
use openidconnect::{
	ClientId, ClientSecret, CsrfToken, Nonce, PkceCodeChallenge, RedirectUrl,
	core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SESSION_COOKIE: &str = "__Host-aidash-session";
const LOGIN_COOKIE: &str = "__Host-aidash-login";
const CSRF_COOKIE: &str = "aidash-csrf";
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

async fn provider_metadata(config: &OidcConfig) -> Result<CoreProviderMetadata> {
	aidash_integrations::oidc::provider_metadata(&config.issuer)
		.await
		.map_err(Into::into)
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

async fn account_valid(
	f: &Federation,
	id: Uuid,
	issuer: &str,
	subject: &str,
	last_valid_at: Option<DateTime<Utc>>,
	disabled_at: Option<DateTime<Utc>>,
) -> Result<()> {
	crate::bootstrap::dashboard_authority(f)
		.account_valid(&aidash_domain::identity::dashboard::Account {
			id,
			issuer: issuer.to_owned(),
			subject: subject.to_owned(),
			last_valid_at,
			disabled_at,
		})
		.await
		.map_err(Into::into)
}

/// Browser sessions and ongoing runs use the same application account authority.
pub async fn refresh_active(
	f: Federation,
	stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	aidash_runtime::dashboard::refresh_active(
		std::sync::Arc::new(crate::bootstrap::dashboard_authority(&f)),
		stopping,
	)
	.await
	.map_err(Into::into)
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
	ApprovedMapping, BackchannelLogout, CallbackQuery, Configuration, IdentityView, LoginQuery,
	MappingRevision, OperatorGrantInput, Registration, SessionView,
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
		let identity = crate::bootstrap::dashboard_authority(&f)
			.admit_login(
				&mut crate::bootstrap::dashboard_login(lease.handle()),
				&subject,
			)
			.await?;
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
		crate::bootstrap::dashboard_authority(&self.runtime)
			.restore(id)
			.await?;
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
		required_config(&f)?;
		let _admission = BACKCHANNEL_CAPACITY
			.try_acquire()
			.map_err(|_| Error::RateLimited)?;
		aidash_application::authorization::dashboard::backchannel_logout(
			&crate::bootstrap::dashboard_logout(&f),
			&body.logout_token,
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
	aidash_integrations::oidc::exchange_identity(
		&crate::bootstrap::oidc_settings(config),
		metadata,
		&aidash_integrations::oidc::PendingLogin {
			callback_uri: &transaction.callback_uri,
			pkce_verifier: &transaction.pkce_verifier,
			nonce: &transaction.nonce,
		},
		code,
	)
	.await
	.map_err(Into::into)
}

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
