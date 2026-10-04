//! OIDC discovery, account lookup and signed token verification are external adapters.
use crate::{Error, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use openidconnect::{
	AuthType, AuthorizationCode, ClientId, ClientSecret, IssuerUrl, Nonce, PkceCodeVerifier,
	RedirectUrl, TokenResponse,
	core::{CoreClient, CoreProviderMetadata},
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, sync::OnceLock, time::Instant};
use tokio::sync::Mutex;

const DISCOVERY_CACHE_SECONDS: u64 = 300;
const JWKS_CACHE_SECONDS: u64 = 300;
const UNKNOWN_KID_REFRESH_SECONDS: u64 = 30;
type DiscoveryCache = Mutex<HashMap<String, (Instant, CoreProviderMetadata)>>;
static DISCOVERY_CACHE: OnceLock<DiscoveryCache> = OnceLock::new();
type JwksCache = Mutex<HashMap<String, (Instant, JwkSet)>>;
static JWKS_CACHE: OnceLock<JwksCache> = OnceLock::new();

/// Connection settings contain secrets and deliberately omit Debug.
#[derive(Clone)]
pub struct Settings {
	pub issuer: String,
	pub client_id: String,
	pub client_secret: String,
	pub keycloak_admin_url: String,
	pub status_client_id: String,
	pub status_client_secret: String,
}
impl Settings {
	fn is_google(&self) -> bool {
		self.issuer == "https://accounts.google.com"
	}
}
pub struct PendingLogin<'a> {
	pub callback_uri: &'a str,
	pub pkce_verifier: &'a str,
	pub nonce: &'a str,
}

pub struct AccountLookup {
	pub client: reqwest::Client,
	pub settings: Option<Settings>,
}
#[async_trait::async_trait]
impl aidash_application::ports::authorization::dashboard::AccountStatus for AccountLookup {
	async fn enabled(&self, subject: &str) -> Result<bool> {
		let settings = self
			.settings
			.as_ref()
			.ok_or_else(|| Error::NotFound("dashboard sign-in is not configured".into()))?;
		keycloak_enabled(&self.client, settings, subject).await
	}
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
#[derive(Deserialize)]
struct GoogleTokenResponse {
	id_token: String,
}
#[derive(Deserialize)]
struct GoogleClaims {
	sub: String,
	nonce: String,
	#[serde(rename = "iat")]
	_issued_at: i64,
	sid: Option<String>,
}

fn oidc_http_client() -> Result<openidconnect::reqwest::Client> {
	openidconnect::reqwest::Client::builder()
		.redirect(openidconnect::reqwest::redirect::Policy::none())
		.timeout(std::time::Duration::from_secs(10))
		.build()
		.map_err(|_| Error::External("OIDC client unavailable".into()))
}

pub async fn provider_metadata(issuer: &str) -> Result<CoreProviderMetadata> {
	let cache = DISCOVERY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
	let mut entries = cache.lock().await;
	if let Some((fetched_at, metadata)) = entries.get(issuer)
		&& fetched_at.elapsed().as_secs() < DISCOVERY_CACHE_SECONDS
	{
		return Ok(metadata.clone());
	}
	let issuer_url = IssuerUrl::new(issuer.to_owned())
		.map_err(|_| Error::Invalid("invalid OIDC issuer".into()))?;
	let metadata = CoreProviderMetadata::discover_async(issuer_url, &oidc_http_client()?)
		.await
		.map_err(|_| Error::External("OIDC discovery unavailable".into()))?;
	entries.insert(issuer.to_owned(), (Instant::now(), metadata.clone()));
	Ok(metadata)
}

pub async fn logout_decoding_key(
	client: &reqwest::Client,
	issuer: &str,
	kid: &str,
) -> Result<DecodingKey> {
	let cache = JWKS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
	let mut entries = cache.lock().await;
	if let Some((fetched_at, keys)) = entries.get(issuer) {
		let age = fetched_at.elapsed().as_secs();
		if age < JWKS_CACHE_SECONDS && keys.find(kid).is_some() {
			return decoding_key(keys, kid);
		}
		// An arbitrary kid must not force a provider request on every attempt.
		if age < UNKNOWN_KID_REFRESH_SECONDS {
			return Err(Error::Invalid("unknown logout signing key".into()));
		}
	}
	let metadata = provider_metadata(issuer).await?;
	let response = client
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
	entries.insert(issuer.to_owned(), (Instant::now(), keys));
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

pub async fn keycloak_enabled(
	client: &reqwest::Client,
	config: &Settings,
	subject: &str,
) -> Result<bool> {
	let token_url = format!(
		"{}/protocol/openid-connect/token",
		config.issuer.trim_end_matches('/')
	);
	let response = client
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
	let response = client
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

pub async fn exchange_identity(
	config: &Settings,
	metadata: CoreProviderMetadata,
	transaction: &PendingLogin<'_>,
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
		RedirectUrl::new(transaction.callback_uri.to_owned()).map_err(|_| Error::Unauthorized)?,
	);
	let client = client.set_auth_type(AuthType::BasicAuth);
	let token_response = client
		.exchange_code(AuthorizationCode::new(code))
		.map_err(|_| Error::Unauthorized)?
		.set_pkce_verifier(PkceCodeVerifier::new(transaction.pkce_verifier.to_owned()))
		.request_async(&http_client)
		.await
		.map_err(|_| Error::Unauthorized)?;
	let id_token = token_response.id_token().ok_or(Error::Unauthorized)?;
	let claims = id_token
		.claims(
			&client.id_token_verifier(),
			&Nonce::new(transaction.nonce.to_owned()),
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
	config: &Settings,
	metadata: &CoreProviderMetadata,
	transaction: &PendingLogin<'_>,
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
			("redirect_uri", transaction.callback_uri),
			("code_verifier", transaction.pkce_verifier),
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
	if !aidash_domain::configuration::same_secret(&claims.nonce, &transaction.nonce) {
		return Err(Error::Unauthorized);
	}
	Ok((claims.sub, claims.sid))
}

/// Verify the signed envelope before application-level logout claim rules.
pub async fn verified_logout(
	client: &reqwest::Client,
	issuer: &str,
	client_id: &str,
	token: &str,
) -> Result<Value> {
	if token.len() > 16_384 {
		return Err(Error::Invalid("invalid logout token".into()));
	}
	let header = decode_header(token).map_err(|_| Error::Invalid("invalid logout token".into()))?;
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
	let decoding_key = logout_decoding_key(client, issuer, &kid).await?;
	let mut validation = Validation::new(Algorithm::RS256);
	validation.set_issuer(&[issuer]);
	validation.set_audience(&[client_id]);
	validation.set_required_spec_claims(&["iss", "aud", "iat", "exp"]);
	validation.leeway = 30;
	let claims = decode::<Value>(token, &decoding_key, &validation)
		.map_err(|_| Error::Invalid("invalid logout token".into()))?
		.claims;
	Ok(claims)
}
