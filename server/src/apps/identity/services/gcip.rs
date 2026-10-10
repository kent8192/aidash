//! Browser-bound GCIP sign-in exchanges reuse dashboard authorization and sessions.
use super::oidc::{self, CSRF_COOKIE, LOGIN_COOKIE, SESSION_COOKIE};
use crate::apps::identity::{
	models::{DashboardLoginTransaction, DashboardSession, byte_key::ByteKey},
	serializers::oidc::LoginQuery,
};
use crate::{Error, Result, federation::Federation};
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
}

pub(crate) async fn login(
	f: &Federation,
	headers: HeaderMap,
	query: LoginQuery,
) -> Result<Response> {
	let config = f.config.gcip.as_ref().ok_or(Error::Unauthorized)?;
	let session_config = oidc::required_config(f)?;
	let destination = oidc::return_path(query.return_to.as_deref())?;
	let Some(org) = query.org.as_deref() else {
		let query = reqwest::Url::parse_with_params(
			"http://localhost/sign-in",
			[("return_to", destination)],
		)
		.map_err(|_| Error::Invalid("invalid sign-in destination".into()))?;
		let mut response = Response::new(StatusCode::SEE_OTHER)
			.with_location(&format!("/sign-in?{}", query.query().unwrap_or("")));
		oidc::no_store(&mut response);
		return Ok(response);
	};
	let tenant = config
		.tenant_bindings
		.iter()
		.find_map(|(pool, tenant)| (tenant == org).then_some(pool))
		.ok_or(Error::Forbidden)?;
	let browser = oidc::cookie_value(&headers, &oidc::cookie_name(LOGIN_COOKIE, session_config))
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
		.gcip_tenant(Some(tenant.clone()))
		.finish();
	let lease = f.store.orm_connection()?;
	transaction.reserve(lease.handle()).await?;
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
	let lease = f.store.orm_connection()?;
	let transaction = DashboardLoginTransaction::objects()
		.filter(DashboardLoginTransaction::field_state_hash().eq(ByteKey(oidc::digest(state))))
		.all_with_db(&mut lease.handle())
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
		providers: f
			.config
			.dashboard_policy()
			.ok_or(Error::Unauthorized)?
			.providers_for(tenant),
		password_sign_up: config.password_sign_up.contains(tenant),
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
	let sign_in = services
		.verifier
		.verify(
			&input.id_token,
			tenant,
			transaction.started_at.ok_or(Error::Unauthorized)?,
		)
		.await?;
	// Verify the token before the shared authority checks the Binding and
	// durably disables an existing identity whose Binding has been removed.
	let lease = f.store.orm_connection()?;
	let identity = crate::bootstrap::dashboard_authority(f)
		.admit_login(
			&mut crate::bootstrap::dashboard_login(lease.handle()),
			&sign_in,
		)
		.await?;
	let session_config = oidc::required_config(f)?;
	let browser = oidc::cookie_value(&headers, &oidc::cookie_name(LOGIN_COOKIE, session_config))
		.ok_or(Error::Unauthorized)?;
	let transaction = DashboardLoginTransaction::consume_bound(
		lease.handle(),
		oidc::digest(&input.state),
		oidc::digest(browser),
	)
	.await?;
	let secret = oidc::random_secret();
	let csrf = oidc::random_secret();
	let previous = oidc::cookie_value(&headers, &oidc::cookie_name(SESSION_COOKIE, session_config))
		.map(oidc::digest);
	DashboardSession::start(
		lease.handle(),
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
