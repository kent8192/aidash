//! GCIP transport adapters. Signed client tokens authenticate; custom claims never authorize.
use crate::{Error, Result};
use aidash_domain::identity::dashboard::{AccountState, SignIn};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use std::{
	sync::Arc,
	time::{Duration, Instant},
};
use tokio::sync::{Mutex, OnceCell};

pub const JWKS_URL: &str =
	"https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com";
pub const AUTH_CLOCK_SKEW_SECONDS: i64 = 30;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[async_trait]
pub trait SigningKeys: Send + Sync {
	async fn key(&self, kid: &str) -> Result<DecodingKey>;
}

struct CachedKeys {
	fetched: Instant,
	lifetime: Duration,
	keys: JwkSet,
}
pub struct HttpSigningKeys {
	client: reqwest::Client,
	url: String,
	cache: Mutex<Option<CachedKeys>>,
}
impl HttpSigningKeys {
	pub fn new(client: reqwest::Client, url: String) -> Self {
		Self {
			client,
			url,
			cache: Mutex::new(None),
		}
	}
}
#[async_trait]
impl SigningKeys for HttpSigningKeys {
	async fn key(&self, kid: &str) -> Result<DecodingKey> {
		let mut cache = self.cache.lock().await;
		if let Some(cached) = cache.as_ref() {
			let age = cached.fetched.elapsed();
			if age < cached.lifetime && cached.keys.find(kid).is_some() {
				return key_from_set(&cached.keys, kid);
			}
			// Untrusted key IDs cannot force network traffic on every attempt.
			if age < Duration::from_secs(30) && cached.keys.find(kid).is_none() {
				return Err(Error::Unauthorized);
			}
		}
		let response = self
			.client
			.get(&self.url)
			.timeout(IO_TIMEOUT)
			.send()
			.await
			.map_err(|_| Error::External("GCIP signing keys unavailable".into()))?;
		if !response.status().is_success() {
			return Err(Error::External("GCIP signing keys unavailable".into()));
		}
		let lifetime = response
			.headers()
			.get(reqwest::header::CACHE_CONTROL)
			.and_then(|h| h.to_str().ok())
			.and_then(|h| {
				h.split(',').find_map(|part| {
					part.trim()
						.strip_prefix("max-age=")
						.and_then(|s| s.parse::<u64>().ok())
				})
			})
			.unwrap_or(300)
			.min(86_400);
		let keys: JwkSet = serde_json::from_slice(&bounded_body(response).await?)
			.map_err(|_| Error::External("GCIP signing keys unavailable".into()))?;
		let result = key_from_set(&keys, kid);
		*cache = Some(CachedKeys {
			fetched: Instant::now(),
			lifetime: Duration::from_secs(lifetime),
			keys,
		});
		result
	}
}
fn key_from_set(keys: &JwkSet, kid: &str) -> Result<DecodingKey> {
	let key = keys.find(kid).ok_or(Error::Unauthorized)?;
	if key
		.common
		.key_algorithm
		.as_ref()
		.is_some_and(|alg| alg.to_string() != "RS256")
	{
		return Err(Error::Unauthorized);
	}
	DecodingKey::from_jwk(key).map_err(|_| Error::Unauthorized)
}
async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>> {
	let mut bytes = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| Error::External("GCIP response unavailable".into()))?
	{
		if bytes.len() + chunk.len() > 1_048_576 {
			return Err(Error::External("GCIP response too large".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

#[derive(Deserialize)]
struct FirebaseClaims {
	tenant: String,
	sign_in_provider: String,
}
#[derive(Deserialize)]
struct Claims {
	sub: String,
	iat: i64,
	exp: i64,
	auth_time: i64,
	firebase: FirebaseClaims,
	#[serde(default)]
	email_verified: bool,
	email: Option<String>,
	name: Option<String>,
}
pub struct TokenVerifier {
	pub project: String,
	pub keys: Arc<dyn SigningKeys>,
}
impl TokenVerifier {
	/// The tenant is selected by a browser-bound server transaction, never by the token.
	pub async fn verify(
		&self,
		token: &str,
		tenant: &str,
		started: DateTime<Utc>,
	) -> Result<SignIn> {
		if token.len() > 16_384 {
			return Err(Error::Unauthorized);
		}
		let header = decode_header(token).map_err(|_| Error::Unauthorized)?;
		if header.alg != Algorithm::RS256 {
			return Err(Error::Unauthorized);
		}
		let kid = header
			.kid
			.filter(|kid| !kid.is_empty() && kid.len() <= 256)
			.ok_or(Error::Unauthorized)?;
		let key = self.keys.key(&kid).await?;
		let mut validation = Validation::new(Algorithm::RS256);
		validation.set_issuer(&[format!("https://securetoken.google.com/{}", self.project)]);
		validation.set_audience(&[&self.project]);
		validation.set_required_spec_claims(&["iss", "aud", "exp", "iat", "sub"]);
		validation.leeway = 0;
		let claims = decode::<Claims>(token, &key, &validation)
			.map_err(|_| Error::Unauthorized)?
			.claims;
		let now = Utc::now().timestamp();
		if claims.sub.is_empty()
			|| claims.sub.len() > 128
			|| claims.firebase.tenant != tenant
			|| claims.iat > now + AUTH_CLOCK_SKEW_SECONDS
			|| claims.exp <= claims.iat
			|| claims.auth_time > now + AUTH_CLOCK_SKEW_SECONDS
			|| claims.auth_time < started.timestamp() - AUTH_CLOCK_SKEW_SECONDS
			|| claims.auth_time > claims.iat + AUTH_CLOCK_SKEW_SECONDS
			|| (claims.firebase.sign_in_provider == "password" && !claims.email_verified)
		{
			return Err(Error::Unauthorized);
		}
		let auth_time = DateTime::from_timestamp(claims.auth_time, 0).ok_or(Error::Unauthorized)?;
		let verified_email = claims
			.email
			.filter(|email| claims.email_verified && !email.is_empty() && email.len() <= 320);
		let display_name = claims
			.name
			.filter(|name| !name.is_empty() && name.len() <= 512);
		Ok(SignIn {
			subject: claims.sub,
			gcip_tenant: Some(tenant.to_owned()),
			auth_time,
			verified_email,
			display_name,
		})
	}
}

#[async_trait]
pub trait AccessToken: Send + Sync {
	async fn token(&self) -> Result<String>;
}
#[derive(Default)]
pub struct ApplicationDefaultCredentials {
	provider: OnceCell<Arc<dyn gcp_auth::TokenProvider>>,
}
#[async_trait]
impl AccessToken for ApplicationDefaultCredentials {
	async fn token(&self) -> Result<String> {
		let provider = self
			.provider
			.get_or_try_init(|| async {
				gcp_auth::provider()
					.await
					.map_err(|_| Error::External("GCIP credentials unavailable".into()))
			})
			.await?;
		let token = provider
			.token(&["https://www.googleapis.com/auth/identitytoolkit"])
			.await
			.map_err(|_| Error::External("GCIP credentials unavailable".into()))?;
		Ok(token.as_str().to_owned())
	}
}

pub struct AccountLookup {
	pub client: reqwest::Client,
	pub project: String,
	pub endpoint: String,
	pub credentials: Arc<dyn AccessToken>,
}
#[derive(Deserialize)]
struct Lookup {
	#[serde(default)]
	users: Vec<User>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
	local_id: String,
	#[serde(default)]
	disabled: bool,
	valid_since: Option<String>,
	tenant_id: Option<String>,
}
#[async_trait]
impl aidash_application::ports::authorization::dashboard::AccountStatus for AccountLookup {
	async fn lookup(&self, subject: &str, gcip_tenant: Option<&str>) -> Result<AccountState> {
		let tenant = gcip_tenant.ok_or(Error::Forbidden)?;
		tokio::time::timeout(IO_TIMEOUT, async {
			let token = self.credentials.token().await?;
			let mut url = reqwest::Url::parse(&self.endpoint)
				.map_err(|_| Error::Invalid("invalid GCIP API endpoint".into()))?;
			url.path_segments_mut()
				.map_err(|_| Error::Invalid("invalid GCIP API endpoint".into()))?
				.extend([
					"v1",
					"projects",
					&self.project,
					"tenants",
					tenant,
					"accounts:lookup",
				]);
			let response = self
				.client
				.post(url)
				.bearer_auth(token)
				.json(&serde_json::json!({"localId":[subject]}))
				.timeout(IO_TIMEOUT)
				.send()
				.await
				.map_err(|_| Error::External("GCIP account status unavailable".into()))?;
			if !response.status().is_success() {
				return Err(Error::External("GCIP account status unavailable".into()));
			}
			let lookup: Lookup = serde_json::from_slice(&bounded_body(response).await?)
				.map_err(|_| Error::External("GCIP account status unavailable".into()))?;
			if lookup.users.is_empty() {
				return Ok(AccountState {
					disabled: true,
					valid_since: None,
				});
			}
			if lookup.users.len() != 1 {
				return Err(Error::External("GCIP account status unavailable".into()));
			}
			let user = &lookup.users[0];
			if user.local_id != subject || user.tenant_id.as_deref().is_some_and(|id| id != tenant)
			{
				return Err(Error::External("GCIP account status unavailable".into()));
			}
			let valid_since = user
				.valid_since
				.as_deref()
				.map(|value| {
					value
						.parse::<i64>()
						.ok()
						.and_then(|value| DateTime::from_timestamp(value, 0))
						.ok_or_else(|| Error::External("GCIP account status unavailable".into()))
				})
				.transpose()?;
			Ok(AccountState {
				disabled: user.disabled,
				valid_since,
			})
		})
		.await
		.map_err(|_| Error::External("GCIP account status unavailable".into()))?
	}
}

/// Shared by HTTP exchanges and account refresh. Tests replace either transport.
pub struct Services {
	pub verifier: TokenVerifier,
	pub status: Arc<dyn aidash_application::ports::authorization::dashboard::AccountStatus>,
}
impl Services {
	pub fn new(project: String, client: reqwest::Client) -> Self {
		Self {
			verifier: TokenVerifier {
				project: project.clone(),
				keys: Arc::new(HttpSigningKeys::new(client.clone(), JWKS_URL.into())),
			},
			status: Arc::new(AccountLookup {
				client,
				project,
				endpoint: "https://identitytoolkit.googleapis.com".into(),
				credentials: Arc::new(ApplicationDefaultCredentials::default()),
			}),
		}
	}
}

#[cfg(test)]
mod tests;
