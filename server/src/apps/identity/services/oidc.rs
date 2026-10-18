//! Backend-owned OIDC login and opaque dashboard sessions.
use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationCredential, DashboardDeploymentAdministratorGrant,
	DashboardIdentity, DashboardLoginTransaction, DashboardMapping, DashboardRegistrationRequest,
	DashboardSession, database_time,
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

use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use chrono::{Duration, Utc};
use openidconnect::{
	ClientId, ClientSecret, CsrfToken, Nonce, PkceCodeChallenge, RedirectUrl,
	core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) const SESSION_COOKIE: &str = "__Host-aidash-session";
pub(crate) const LOGIN_COOKIE: &str = "__Host-aidash-login";
pub(crate) const CSRF_COOKIE: &str = "aidash-csrf";
static BACKCHANNEL_CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

pub(crate) fn digest(value: &str) -> Vec<u8> {
	Sha256::digest(value.as_bytes()).to_vec()
}

pub(crate) fn random_secret() -> String {
	// Two independent UUIDv4 values provide 244 random bits.
	format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub(crate) fn required_config(f: &Federation) -> Result<crate::config::SessionConfig<'_>> {
	f.config.dashboard_session().ok_or(Error::NotFound(
		"dashboard sign-in is not configured".into(),
	))
}

pub(crate) fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
	headers
		.get_all(header::COOKIE)
		.iter()
		.filter_map(|value| value.to_str().ok())
		.flat_map(|value| value.split(';'))
		.find_map(|item| {
			let (key, value) = item.trim().split_once('=')?;
			(key == name).then_some(value)
		})
}

#[cfg(test)]
#[path = "../tests/oidc_headers.rs"]
mod header_tests;

fn secure_cookie<'a>(config: impl Into<crate::config::SessionConfig<'a>>) -> bool {
	config.into().public_origin.starts_with("https://")
}

pub(crate) fn cookie_name<'a>(
	base: &str,
	config: impl Into<crate::config::SessionConfig<'a>>,
) -> String {
	if secure_cookie(config) {
		base.to_owned()
	} else {
		base.trim_start_matches("__Host-").to_owned()
	}
}

pub(crate) fn set_cookie<'a>(
	response: &mut Response,
	name: &str,
	value: &str,
	config: impl Into<crate::config::SessionConfig<'a>>,
	http_only: bool,
	max_age: i64,
) -> Result<()> {
	let config = config.into();
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

fn clear_cookie<'a>(
	response: &mut Response,
	name: &str,
	config: impl Into<crate::config::SessionConfig<'a>>,
	http_only: bool,
) -> Result<()> {
	set_cookie(response, name, "", config, http_only, 0)
}

pub(crate) fn no_store(response: &mut Response) {
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

pub(crate) fn return_path(value: Option<&str>) -> Result<&str> {
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

async fn account_valid(f: &Federation, row: DashboardIdentity) -> Result<()> {
	crate::bootstrap::dashboard_authority(f)
		.account_valid(&crate::apps::identity::repositories::dashboard::account(
			row,
		))
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

/// Enforces display retention while dashboard sign-in is not configured.
///
/// [`refresh_active`] enforces it on each pass when sign-in is configured.
/// Without sign-in no session is usable or still starting, so only authority
/// or a pending Registration Request retains Display Attributes.
pub async fn enforce_display_retention(
	f: Federation,
	mut stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
	loop {
		let idle_seconds = f
			.config
			.dashboard_session()
			.map(|config| config.session_idle_seconds);
		let pass = DashboardIdentity::enforce_display_retention(&f.store.pool, idle_seconds)
			.await
			.err();
		if let Some(error) = pass {
			tracing::warn!(%error, "display retention pass failed; retrying after interval");
		}
		tokio::select! {
			_ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {},
			_ = stopping.changed() => if *stopping.borrow() { return Ok(()); },
		}
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
	let session = f
		.store
		.pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			DashboardSession::from_token(tx, digest(secret)).await
		})
		.await?;
	if session.desktop
		|| session.revoked_at.is_some()
		|| session.expires_at <= Utc::now()
		|| session.last_activity_at <= Utc::now() - Duration::seconds(config.session_idle_seconds)
	{
		return Err(Error::Unauthorized);
	}
	Ok(session)
}

pub async fn session_from_headers(f: &Federation, headers: &HeaderMap) -> Result<BrowserSession> {
	let session =
		if let Some(token) = crate::apps::identity::repositories::desktop::access_token(headers) {
			crate::apps::identity::repositories::desktop::session_record(f, token).await?
		} else {
			session_record_from_headers(f, headers).await?
		};
	validate_session_identity(f, session).await
}

pub(crate) async fn browser_session_from_headers(
	f: &Federation,
	headers: &HeaderMap,
) -> Result<BrowserSession> {
	let session = session_record_from_headers(f, headers).await?;
	validate_session_identity(f, session).await
}

pub(crate) async fn validate_session_identity(
	f: &Federation,
	session: BrowserSession,
) -> Result<BrowserSession> {
	let pool = &f.store.pool;
	let identity = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			DashboardIdentity::find(tx, session.identity_id()).await
		})
		.await?;
	account_valid(f, identity).await?;
	// Revalidation may revoke the session. Reread it by id: a concurrent desktop
	// refresh rotates its access-token hash, which is not a revocation.
	let current = pool
		.atomic(&TenancyScope::NoTenant, async |tx| {
			DashboardSession::find(tx, session.id).await
		})
		.await?;
	if current.revoked_at.is_some() {
		return Err(Error::Unauthorized);
	}

	Ok(session)
}

pub fn csrf_allowed<'a>(
	config: impl Into<crate::config::SessionConfig<'a>>,
	headers: &HeaderMap,
	session: &BrowserSession,
	method: &Method,
) -> bool {
	let config = config.into();
	if session.desktop {
		return crate::apps::identity::repositories::desktop::access_token(headers).is_some();
	}
	if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
		return true;
	}
	let origin = headers
		.get(header::ORIGIN)
		.and_then(|value| value.to_str().ok());
	let csrf = headers
		.get("x-aidash-csrf")
		.and_then(|value| value.to_str().ok());
	origin == Some(config.public_origin)
		&& csrf.is_some_and(|value| digest(value) == session.csrf_hash)
}

#[derive(Clone)]
pub struct BrowserOrigin {
	pub identity_id: Uuid,
	pub mapping_id: Option<Uuid>,
	pub(crate) session: identity::HttpSession,
}

impl BrowserOrigin {
	/// A browser session acting without a Mapping: as a Deployment
	/// Administrator, whose grant [`Self::require_deployment_administrator`]
	/// checks, or as the External Identity itself.
	pub(crate) fn unmapped(
		config: crate::config::SessionConfig<'_>,
		session: &BrowserSession,
	) -> Self {
		Self {
			identity_id: session.identity_id(),
			mapping_id: None,
			session: http_session(config, session),
		}
	}
}

fn http_session(
	config: crate::config::SessionConfig<'_>,
	session: &BrowserSession,
) -> identity::HttpSession {
	identity::HttpSession {
		id: session.id,
		identity_id: session.identity_id(),
		idle_seconds: session
			.desktop_idle_seconds
			.unwrap_or(config.session_idle_seconds),
		access_expires_at: session.access_expires_at,
	}
}

pub async fn actor_from_headers(
	f: &Federation,
	headers: &HeaderMap,
	method: &Method,
) -> Result<(Actor, BrowserOrigin)> {
	let config = required_config(f)?;
	let session = session_from_headers(f, headers).await?;
	let http_session = http_session(config, &session);
	if !csrf_allowed(config, headers, &session, method) {
		return Err(Error::Forbidden);
	}
	let selector = headers
		.get("x-aidash-context")
		.and_then(|value| value.to_str().ok())
		.ok_or(Error::Forbidden)?;
	let pool = &f.store.pool;
	if selector == "deployment_administrator" {
		let granted = pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardDeploymentAdministratorGrant::enabled_for_identity(
					tx,
					session.identity_id(),
				)
				.await
			})
			.await?;
		if !granted {
			return Err(Error::Forbidden);
		}
		return Ok((
			Actor::DeploymentAdministrator,
			BrowserOrigin::unmapped(config, &session),
		));
	}
	// The selector names the Mapping's Tenant, so the lookup runs in that
	// Tenant's scope and never reads across Tenants (ADR 0047).
	let (tenant, id) = aidash_domain::identity::credentials::parse_mapping_selector(selector)
		.ok_or(Error::Forbidden)?;
	let (mapping, deleted) = pool
		.atomic(&TenancyScope::Tenant(tenant.clone()), async |tx| {
			let mapping = DashboardMapping::find(tx, id).await?;
			let deleted = crate::apps::identity::repositories::tenant_deletions::is_deleted(
				tx,
				tenant.as_str(),
			)
			.await?;
			Ok((mapping, deleted))
		})
		.await?;
	let mapping = mapping.ok_or(Error::Forbidden)?;
	if mapping.identity_id() != session.identity_id()
		|| !mapping.enabled
		|| mapping.tenant != tenant.as_str()
	{
		return Err(Error::Forbidden);
	}
	// A member's browser learns that its Tenant is deleted; a desktop token,
	// like a Subject Credential, sees what an unknown Tenant would (ADR 0060).
	if deleted {
		return Err(if session.desktop {
			Error::Unauthorized
		} else {
			Error::TenantDeleted
		});
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

/// `/api/dashboard` routes admit only Deployment Administrators: the bearer token or a browser
/// session holding a Deployment Administrator grant.
fn deployment_administrator_actor(
	origin: Option<BrowserOrigin>,
) -> crate::apps::identity::models::AdministrationActor {
	use crate::apps::identity::models::AdministrationActor;
	origin.map_or(
		AdministrationActor::DeploymentAdministratorBearer,
		|origin| AdministrationActor::DeploymentAdministrator {
			identity: origin.identity_id,
		},
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
	AdminIdentityPage, AdminMapping, AdminMappingPage, Approval, ApprovedMapping,
	BackchannelLogout, CallbackQuery, Configuration, DeploymentAdministratorGrant,
	DeploymentAdministratorGrantInput, IdentityView, LoginQuery, MappingRevision, Registration,
	SessionView,
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
			enabled: f.config.dashboard_session().is_some(),
			desktop_protocol: 2,
			provider: if f.config.gcip.is_some() {
				"gcip"
			} else if f.config.oidc.as_ref().is_none_or(OidcConfig::is_google) {
				"google"
			} else {
				"keycloak"
			},
			login_url: f.config.dashboard_session().map(|_| "/auth/login"),
		})
	}
	pub(crate) async fn login(&self, headers: HeaderMap, query: LoginQuery) -> Result<Response> {
		let f = self.runtime.clone();
		if f.config.gcip.is_some() {
			return crate::apps::identity::services::gcip::login(&f, headers, query).await;
		}
		let config = f.config.oidc.as_ref().ok_or(Error::Unauthorized)?;
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
			.started_at(None)
			.gcip_tenant(None)
			.finish();
		let pool = &f.store.pool;
		reservation.reserve(pool).await?;
		let metadata = match provider_metadata(config).await {
			Ok(metadata) => metadata,
			Err(error) => {
				if let Err(cleanup_error) = pool
					.atomic(&TenancyScope::NoTenant, async |tx| {
						DashboardLoginTransaction::release(tx, state_hash).await
					})
					.await
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
		let config = f.config.oidc.as_ref().ok_or(Error::Unauthorized)?;
		let browser = cookie_value(&headers, &cookie_name(LOGIN_COOKIE, config))
			.ok_or(Error::Unauthorized)?;
		let pool = &f.store.pool;
		let transaction = DashboardLoginTransaction::consume(pool, digest(&query.state)).await?;
		if transaction.expires_at <= Utc::now()
			|| transaction.browser_hash != digest(browser)
			|| transaction.callback_uri != format!("{}/auth/callback", config.public_origin)
		{
			return Err(Error::Unauthorized);
		}
		let metadata = provider_metadata(config).await?;
		let (sign_in, provider_sid) =
			exchange_identity(config, metadata, &transaction, query.code).await?;
		let identity = crate::bootstrap::dashboard_authority(&f)
			.admit_login(
				&mut crate::bootstrap::dashboard_login(pool.clone()),
				&sign_in,
			)
			.await?;
		let secret = random_secret();
		let csrf = random_secret();
		let previous = cookie_value(&headers, &cookie_name(SESSION_COOKIE, config)).map(digest);
		DashboardSession::start(
			pool,
			identity.id,
			(digest(&secret), digest(&csrf)),
			provider_sid,
			sign_in.auth_time,
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
		let pool = &f.store.pool;
		let external = pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::find(tx, session.identity_id()).await
			})
			.await?;
		let policy = f.config.dashboard_policy();
		// Existing sessions receive the same one-time provisioning as a fresh
		// verified callback, without changing pending shared enrollment.
		let gcip_pool = (!external.gcip_tenant.is_empty()).then_some(external.gcip_tenant.as_str());
		if policy
			.as_ref()
			.is_some_and(|policy| policy.provisions_personal(gcip_pool))
		{
			crate::apps::identity::repositories::personal_tenant::provision(pool, external.id)
				.await?;
		}
		// The identity's Mappings in every Tenant, which the Tenant switcher lists.
		let scope = TenancyScope::Deployment(DeploymentScopeReason::IdentityTenants);
		let (
			personal,
			mappings,
			active,
			deployment_administrator,
			labels,
			deleted_tenants,
			personal_tenant_deleted,
		) = pool
			.atomic(&scope, async |tx| {
				let personal =
					crate::apps::identity::repositories::personal_tenant::of(tx, external.id)
						.await?;
				let mappings = DashboardMapping::enabled_for_identity(tx, external.id)
					.await?
					.into_iter()
					.filter(|mapping| {
						identity::require_dashboard_mapping(
							policy.as_ref(),
							&external.issuer,
							&external.gcip_tenant,
							&mapping.tenant,
						)
						.is_ok()
					})
					.collect::<Vec<_>>();
				let active = AuthorizationCredential::active_ids(
					tx,
					mappings
						.iter()
						.map(|mapping| mapping.credential_id)
						.collect(),
				)
				.await?;
				let deployment_administrator =
					DashboardDeploymentAdministratorGrant::enabled_for_identity(
						tx,
						session.identity_id(),
					)
					.await?;
				let labels = crate::apps::identity::repositories::tenants::labels(
					tx,
					mappings
						.iter()
						.map(|mapping| mapping.tenant.clone())
						.collect(),
				)
				.await?;
				let mut tenants: Vec<String> = mappings
					.iter()
					.map(|mapping| mapping.tenant.clone())
					.collect();
				tenants.extend(personal.clone());
				let deletions =
					crate::apps::identity::repositories::tenant_deletions::current_of(tx, tenants)
						.await?;
				let mut deleted = Vec::new();
				let now = database_time(tx).await?;
				// One read for the policies of every deleted Tenant.
				let bundles = if deletions.is_empty() {
					std::collections::BTreeMap::new()
				} else {
					<AuthorizationBundle as reinhardt::db::orm::Model>::objects()
						.filter(
							AuthorizationBundle::field_tenant()
								.is_in(deletions.keys().cloned().collect::<Vec<_>>()),
						)
						.all_with_db(tx)
						.await?
						.into_iter()
						.filter_map(|row| {
							serde_json::from_value::<aidash_domain::policy::PolicyBundle>(
								row.document.0,
							)
							.ok()
							.map(|bundle| (row.tenant, bundle))
						})
						.collect()
				};
				for mapping in mappings
					.iter()
					.filter(|m| deletions.contains_key(&m.tenant))
				{
					let deletion = &deletions[&mapping.tenant];
					let bundle = bundles.get(&mapping.tenant);
					deleted.push(
						crate::apps::identity::serializers::tenants::DeletedTenantView {
							tenant: mapping.tenant.clone(),
							label: labels.get(&mapping.tenant).cloned().flatten(),
							mapping_id: mapping.id,
							personal: personal.as_deref() == Some(mapping.tenant.as_str()),
							purge_after: deletion.purge_after,
							// Restoring locks the Mapping's credential as valid.
							restorable: active.contains(&mapping.credential_id)
								&& bundle.is_some_and(|bundle| {
									may_restore(bundle, &mapping.subject, &f.config.node_id)
								}) && deletion.rule().is_ok_and(|rule| {
								aidash_domain::identity::tenancy::deletion::restorable(
								&rule,
								aidash_domain::identity::tenancy::deletion::Actor::TenantAdministrator,
								now,
							)
							}),
						},
					);
				}
				let personal_deleted = personal
					.as_ref()
					.is_some_and(|tenant| deletions.contains_key(tenant));
				let mappings = mappings
					.into_iter()
					.filter(|mapping| !deletions.contains_key(&mapping.tenant))
					.collect::<Vec<_>>();
				Ok((
					personal,
					mappings,
					active,
					deployment_administrator,
					labels,
					deleted,
					personal_deleted,
				))
			})
			.await?;
		let mut response = Response::ok().with_json(
			&(SessionView {
				id: session.id,
				personal_account: personal.is_some(),
				tenant_creation: crate::apps::identity::services::tenants::creation_offered(
					&f,
					gcip_pool,
					external.disabled_at.is_some(),
				),
				invitations: f.config.tenancy.invitations,
				deleted_tenants,
				// Asking for a new one needs the Tenancy Rules that provide one.
				personal_tenant_deleted: personal_tenant_deleted
					&& policy
						.as_ref()
						.is_some_and(|policy| policy.provisions_personal(gcip_pool)),
				deployment_administrator,
				mappings: mappings
					.into_iter()
					.map(|row| MappingView {
						id: row.id,
						personal: personal.as_deref() == Some(row.tenant.as_str())
							&& row.subject == aidash_domain::identity::tenancy::OWNER,
						credential_active: active.contains(&row.credential_id),
						label: labels.get(&row.tenant).cloned().flatten(),
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
		let idle_seconds = required_config(&f)?.session_idle_seconds;
		let session = session_from_headers(&f, &headers).await?;
		let pool = &f.store.pool;
		DashboardIdentity::enforce_identity_retention(
			pool,
			session.identity_id(),
			Some(idle_seconds),
		)
		.await?;
		let latest = pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardRegistrationRequest::latest(tx, session.identity_id()).await
			})
			.await?;
		let mut response =
			Response::ok().with_json(&(latest.map(Registration::with_effective_status)))?;
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
		let pool = &f.store.pool;
		// A Tenant Binding addresses the request to its Tenant; otherwise only
		// Deployment Administrators decide it.
		let identity = pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::find(tx, session.identity_id()).await
			})
			.await?;
		let tenant = f
			.config
			.gcip
			.as_ref()
			.and_then(|gcip| gcip.tenant_bindings.get(&identity.gcip_tenant).cloned());
		DashboardRegistrationRequest::submit(pool, session.identity_id(), tenant).await
	}
	pub(crate) async fn admin_registrations(
		&self,
	) -> Result<Vec<crate::apps::identity::serializers::oidc::PendingRegistration>> {
		DashboardRegistrationRequest::pending(&self.runtime.store.pool).await
	}
	pub(crate) async fn admin_identity(&self, id: Uuid) -> Result<IdentityView> {
		Ok(self
			.runtime
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::find(tx, id).await
			})
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
		self.runtime
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardIdentity::page(tx, page.offset).await
			})
			.await
	}
	pub(crate) async fn admin_mappings(&self, page: AdminMappingPage) -> Result<Vec<AdminMapping>> {
		let scope = TenancyScope::Deployment(DeploymentScopeReason::DeploymentAdministratorList);
		self.runtime
			.store
			.pool
			.atomic(&scope, async |tx| {
				DashboardMapping::page(tx, page.offset).await
			})
			.await
	}
	pub(crate) async fn admin_approve(
		&self,
		actor: Option<BrowserOrigin>,
		id: Uuid,
		input: Approval,
	) -> Result<ApprovedMapping> {
		validate(&input)?;
		let actor = deployment_administrator_actor(actor);
		let approve = || {
			DashboardRegistrationRequest::approve(
				&self.runtime.store.pool,
				id,
				Approval {
					tenant: input.tenant.clone(),
					subject: input.subject.clone(),
					groups: input.groups.clone(),
				},
				Default::default(),
				&actor,
				digest(&random_secret()),
				self.runtime.config.dashboard_policy(),
			)
		};
		let result = approve().await;
		// As for a Tenant Administrator: a previous owner's connection is
		// retired only for an approval refused for that alone (ADR 0007).
		if result.as_ref().is_err_and(
			crate::apps::identity::repositories::plan_connections::refused_shared_holder,
		) {
			crate::apps::identity::services::plan_connections::retire_unmapped(
				&self.runtime,
				&input.tenant,
				&input.subject,
				&actor.decision_actor(),
			)
			.await?;
			return approve().await;
		}
		result
	}
	pub(crate) async fn admin_reject(
		&self,
		actor: Option<BrowserOrigin>,
		id: Uuid,
	) -> Result<Registration> {
		// Bearer Deployment Administrators may reject requests left from a removed sign-in configuration.
		let idle_seconds = self
			.runtime
			.config
			.dashboard_session()
			.map(|config| config.session_idle_seconds);
		DashboardRegistrationRequest::reject(
			&self.runtime.store.pool,
			id,
			&deployment_administrator_actor(actor),
			idle_seconds,
		)
		.await
	}
	pub(crate) async fn admin_erase_display(
		&self,
		actor: Option<BrowserOrigin>,
		id: Uuid,
	) -> Result<IdentityView> {
		DashboardIdentity::erase_display(
			&self.runtime.store.pool,
			id,
			deployment_administrator_actor(actor).decision_actor(),
		)
		.await
	}
	pub(crate) async fn deployment_administrator_grant(
		&self,
		id: Uuid,
		input: DeploymentAdministratorGrantInput,
	) -> Result<DeploymentAdministratorGrant> {
		validate(&input)?;
		DashboardDeploymentAdministratorGrant::change_enabled(
			&self.runtime.store.pool,
			id,
			input.enabled,
			input.expected_revision,
		)
		.await
	}
	pub(crate) async fn deployment_administrator_grants(
		&self,
	) -> Result<Vec<DeploymentAdministratorGrant>> {
		self.runtime
			.store
			.pool
			.atomic(&TenancyScope::NoTenant, async |tx| {
				DashboardDeploymentAdministratorGrant::list(tx).await
			})
			.await
	}
	pub(crate) async fn admin_disable_mapping(
		&self,
		actor: Option<BrowserOrigin>,
		id: Uuid,
		input: MappingRevision,
	) -> Result<http::StatusCode> {
		validate(&input)?;
		let actor = deployment_administrator_actor(actor);
		DashboardMapping::disable(
			&self.runtime.store.pool,
			id,
			input.expected_revision,
			&actor,
		)
		.await?;
		crate::apps::identity::services::plan_connections::retire_disabled_mapping(
			&self.runtime,
			id,
			&actor.decision_actor(),
		)
		.await;
		Ok(http::StatusCode::NO_CONTENT)
	}
	pub(crate) async fn logout(&self, headers: HeaderMap) -> Result<Response> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let session = if let Some(token) =
			crate::apps::identity::repositories::desktop::access_token(&headers)
		{
			crate::apps::identity::repositories::desktop::session_record(&f, token).await?
		} else {
			session_record_from_headers(&f, &headers).await?
		};
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		DashboardSession::revoke(&f.store.pool, session.id, false).await?;
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
		DashboardSession::record_activity(&f.store.pool, session.id).await?;
		Ok(http::StatusCode::NO_CONTENT)
	}
	pub(crate) async fn logout_all(&self, headers: HeaderMap) -> Result<Response> {
		let f = self.runtime.clone();
		let config = required_config(&f)?;
		let session = if let Some(token) =
			crate::apps::identity::repositories::desktop::access_token(&headers)
		{
			crate::apps::identity::repositories::desktop::session_record(&f, token).await?
		} else {
			session_record_from_headers(&f, &headers).await?
		};
		if !csrf_allowed(config, &headers, &session, &Method::POST) {
			return Err(Error::Forbidden);
		}
		DashboardSession::revoke(&f.store.pool, session.identity_id(), true).await?;
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
) -> Result<(aidash_domain::identity::dashboard::SignIn, Option<String>)> {
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
	pub(crate) async fn require_deployment_administrator<E: TransactionExecutor + ?Sized>(
		&self,
		tx: &mut E,
		lock: bool,
	) -> Result<()> {
		if self.mapping_id.is_some() {
			return Err(Error::Forbidden);
		}
		// Deployment Administrator grant writes lock the identity before the grant. Retain that
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
		let status = crate::database::native::query_as(&query.to_string(PostgresQueryBuilder))
			.columns(&["issuer", "last_valid_at", "disabled_at"])
			.bind(self.identity_id)
			.fetch_optional(&mut *tx)
			.await?;
		identity::validate_dashboard_status(status)?;
		let mut query = Query::select();
		query
			.column(Alias::new("enabled"))
			.from(Alias::new("dashboard_deployment_administrator_grants"))
			.and_where(
				reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("identity_id")))
					.eq(Expr::cust("$1")),
			);
		if lock {
			query.lock(LockType::Share);
		}
		let enabled: Option<bool> =
			crate::database::native::query_scalar(&query.to_string(PostgresQueryBuilder))
				.bind(self.identity_id)
				.scalar_optional(&mut *tx)
				.await?;
		if enabled != Some(true) {
			return Err(Error::Forbidden);
		}
		self.session.current(tx, lock).await
	}
}

use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

/// Whether `subject` may restore its deleted Tenant under the policy the
/// Tenant was deleted with, for offering the action; restoring rechecks it in
/// the same environment.
fn may_restore(bundle: &aidash_domain::policy::PolicyBundle, subject: &str, node_id: &str) -> bool {
	bundle
		.subjects
		.get(subject)
		.is_some_and(|entry| entry.enabled)
		&& bundle
			.evaluate(&aidash_domain::policy::Evaluation {
				subject: subject.into(),
				action: aidash_domain::identity::tenancy::TENANT_DELETE.into(),
				resource: aidash_domain::policy::Resource {
					tenant: bundle.tenant.clone(),
					kind: "tenant".into(),
					id: bundle.tenant.clone(),
					attributes: serde_json::json!({}),
				},
				environment: serde_json::json!({"node_id": node_id, "transport": "api"}),
			})
			.allowed
}
