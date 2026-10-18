//! Browser-bound GCIP sign-in exchanges reuse dashboard authorization and sessions.
use super::oidc::{self, CSRF_COOKIE, LOGIN_COOKIE, SESSION_COOKIE};
use crate::apps::identity::{
	models::{DashboardLoginTransaction, DashboardSession, byte_key::ByteKey},
	serializers::oidc::{LoginDiscovery, LoginQuery},
};
use crate::{Error, Result, federation::Federation};
use aidash_application::tenancy::TenancyScope;
use chrono::{Duration, Utc};
use http::{HeaderMap, StatusCode, header};
use reinhardt::db::orm::Model;
use reinhardt::{Response, Validate};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
pub(crate) struct TransactionQuery {
	pub state: String,
}
#[derive(Deserialize, Validate, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Exchange {
	#[validate(length(min = 1, max = 256))]
	pub state: String,
	#[validate(length(min = 1, max = 16384))]
	pub id_token: String,
}
#[derive(Serialize, JsonSchema)]
struct ClientConfig<'a> {
	project_id: &'a str,
	api_key: &'a str,
	auth_domain: String,
	tenant_id: &'a str,
	providers: Vec<String>,
	password_sign_up: bool,
	/// A hint for the sign-in page; GCIP enforces the MFA Requirement.
	mfa_mandatory: bool,
}

/// GCIP Tenants with a Tenant Binding, whose External Identities may be
/// mapped only into their bound Tenant. Any other GCIP Tenant admits nobody.
pub(crate) struct Pools {
	project: String,
	bindings: std::collections::BTreeMap<String, String>,
}

impl Pools {
	pub(crate) fn new(config: &crate::config::GcipConfig) -> Self {
		Self {
			project: config.project_id.clone(),
			bindings: config.tenant_bindings.clone(),
		}
	}
}

impl aidash_application::ports::authorization::dashboard::Issuers for Pools {
	fn admitted(&self) -> Vec<String> {
		self.bindings
			.keys()
			.map(|pool| aidash_integrations::gcip::issuer(&self.project, pool))
			.collect()
	}
	fn admits(&self, issuer: &str) -> bool {
		self.restriction(issuer).is_some()
	}
	fn restriction(&self, issuer: &str) -> Option<String> {
		self.bindings
			.get(aidash_integrations::gcip::tenant(&self.project, issuer)?)
			.cloned()
	}
	fn restricted_issuer(&self, tenant: &str) -> Option<String> {
		self.bindings.iter().find_map(|(pool, bound)| {
			(bound == tenant).then(|| aidash_integrations::gcip::issuer(&self.project, pool))
		})
	}
	fn retires_refused(&self) -> bool {
		true
	}
}

/// `GET /auth/login?org=<Tenant name>` selects the GCIP Tenant explicitly.
pub(crate) async fn login(
	f: &Federation,
	headers: HeaderMap,
	query: LoginQuery,
) -> Result<Response> {
	let config = f.config.gcip.as_ref().ok_or(Error::Unauthorized)?;
	let destination = oidc::return_path(query.return_to.as_deref())?;
	let Some(org) = query.org.as_deref() else {
		return sign_in_page(&[("return_to", destination)]);
	};
	match config
		.tenant_bindings
		.iter()
		.find_map(|(pool, tenant)| (tenant == org).then_some(pool))
	{
		Some(pool) => begin(f, &headers, destination, pool.clone()).await,
		None => not_found(destination),
	}
}
/// `POST /auth/login` routes a verified Sign-in Domain to its GCIP Tenant.
/// Unmapped domains and malformed addresses get the same response as an
/// unknown Tenant name, and nothing about the address is retained.
pub(crate) async fn discover(
	f: &Federation,
	headers: HeaderMap,
	input: LoginDiscovery,
) -> Result<Response> {
	let config = f.config.gcip.as_ref().ok_or(Error::Unauthorized)?;
	if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok())
		!= Some(config.public_origin.as_str())
	{
		return Err(Error::Forbidden);
	}
	let destination = oidc::return_path(input.return_to.as_deref())?;
	match aidash_domain::identity::dashboard::routed_gcip_tenant(
		&config.sign_in_domains,
		&input.email,
	) {
		Some(pool) => begin(f, &headers, destination, pool.to_owned()).await,
		None => not_found(destination),
	}
}
/// The single response for every unknown sign-in input. It never depends on
/// which Tenants or Sign-in Domains exist, and it creates no transaction.
fn not_found(destination: &str) -> Result<Response> {
	sign_in_page(&[("error", "not_found"), ("return_to", destination)])
}
fn sign_in_page(params: &[(&str, &str)]) -> Result<Response> {
	let query = reqwest::Url::parse_with_params("http://localhost/sign-in", params)
		.map_err(|_| Error::Invalid("invalid sign-in destination".into()))?;
	let mut response = Response::new(StatusCode::SEE_OTHER)
		.with_location(&format!("/sign-in?{}", query.query().unwrap_or("")));
	oidc::no_store(&mut response);
	Ok(response)
}
async fn begin(
	f: &Federation,
	headers: &HeaderMap,
	destination: &str,
	tenant: String,
) -> Result<Response> {
	let config = f.config.gcip.as_ref().ok_or(Error::Unauthorized)?;
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
		.callback_uri(format!("{}/auth/gcip/exchange", config.public_origin))
		.expires_at(now + Duration::minutes(10))
		.started_at(Some(now))
		.gcip_tenant(Some(tenant))
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
	oidc::no_store(&mut response);
	Ok(response)
}
async fn transaction(
	f: &Federation,
	headers: &HeaderMap,
	state: &str,
) -> Result<DashboardLoginTransaction> {
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
		|| transaction.callback_uri != format!("{}/auth/gcip/exchange", config.public_origin)
		|| transaction.started_at.is_none()
		|| transaction.gcip_tenant.is_none()
	{
		return Err(Error::Unauthorized);
	}
	Ok(transaction)
}
/// The sign-in methods a GCIP Tenant offers.
fn providers(config: &crate::config::GcipConfig, tenant: &str) -> Vec<String> {
	config
		.providers
		.get(tenant)
		.cloned()
		.unwrap_or_else(|| vec!["google.com".into(), "password".into()])
}

pub(crate) async fn configuration(
	f: &Federation,
	headers: HeaderMap,
	state: String,
) -> Result<Response> {
	let config = f.config.gcip.as_ref().ok_or(Error::Unauthorized)?;
	let transaction = transaction(f, &headers, &state).await?;
	let tenant = transaction
		.gcip_tenant
		.as_deref()
		.ok_or(Error::Unauthorized)?;
	if !config.tenant_bindings.contains_key(tenant) {
		return Err(Error::Forbidden);
	}
	let client = ClientConfig {
		project_id: &config.project_id,
		api_key: &config.web_api_key,
		auth_domain: config.auth_domain(),
		tenant_id: tenant,
		providers: providers(config, tenant),
		password_sign_up: config.password_sign_up.contains(tenant),
		mfa_mandatory: config.mfa_mandatory.contains(tenant),
	};
	let mut response = Response::ok().with_json(&client)?;
	oidc::no_store(&mut response);
	Ok(response)
}
pub(crate) async fn exchange(
	f: &Federation,
	headers: HeaderMap,
	input: Exchange,
) -> Result<Response> {
	crate::http::validate(&input)?;
	let config = f.config.gcip.as_ref().ok_or(Error::Unauthorized)?;
	if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok())
		!= Some(config.public_origin.as_str())
	{
		return Err(Error::Forbidden);
	}
	let transaction = transaction(f, &headers, &input.state).await?;
	let tenant = transaction
		.gcip_tenant
		.as_deref()
		.ok_or(Error::Unauthorized)?;
	let services = f.gcip.as_ref().ok_or(Error::Unauthorized)?;
	if services.verifier.project != config.project_id {
		return Err(Error::Unauthorized);
	}
	let (sign_in, method) = services
		.verifier
		.verify(
			&input.id_token,
			tenant,
			transaction.started_at.ok_or(Error::Unauthorized)?,
		)
		.await?;
	// Verify the token before the shared authority checks the Binding and
	// durably disables an existing identity whose Binding has been removed.
	// The sign-in method is checked only for a bound GCIP Tenant; the
	// authority refuses any other.
	if config.tenant_bindings.contains_key(tenant) && !providers(config, tenant).contains(&method) {
		return Err(Error::Forbidden);
	}
	let identity = crate::bootstrap::dashboard_authority(f)
		.admit_login(
			&mut crate::bootstrap::dashboard_login(f.store.pool.clone(), &f.config.node_id),
			&sign_in,
		)
		.await?;
	let session_config = oidc::required_config(f)?;
	let browser = oidc::cookie_value(&headers, &oidc::cookie_name(LOGIN_COOKIE, session_config))
		.ok_or(Error::Unauthorized)?;
	let transaction = DashboardLoginTransaction::consume_bound(
		&f.store.pool,
		oidc::digest(&input.state),
		oidc::digest(browser),
	)
	.await?;
	let secret = oidc::random_secret();
	let csrf = oidc::random_secret();
	let previous = oidc::cookie_value(&headers, &oidc::cookie_name(SESSION_COOKIE, session_config))
		.map(oidc::digest);
	DashboardSession::start(
		&f.store.pool,
		identity.id,
		(oidc::digest(&secret), oidc::digest(&csrf)),
		None,
		sign_in.auth_time,
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
	oidc::no_store(&mut response);
	Ok(response)
}
