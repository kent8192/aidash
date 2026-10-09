//! Minimal GCP REST adapters. Signing and Key Material reads use separate identities.
pub mod issuer;
use aidash_capability::{KeyMaterialError, KeyMaterialSource, SignError, TokenSigner};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::Instant};
use zeroize::Zeroizing;

/// OAuth access to GCP, distinct from the worker's Capability Token.
#[async_trait]
pub trait AccessTokenSource: Send + Sync {
	async fn token(&self) -> Result<SecretString, SignError>;
}

pub struct MetadataTokenSource {
	client: reqwest::Client,
	url: String,
	cached: Mutex<Option<(SecretString, Instant)>>,
}
impl MetadataTokenSource {
	pub fn new() -> Result<Self, SignError> {
		Ok(Self {
			client: reqwest::Client::builder().no_proxy()
				.redirect(reqwest::redirect::Policy::none())
				.timeout(Duration::from_secs(5)).build().map_err(|_| SignError)?,
			url: "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token".into(),
			cached: Mutex::new(None),
		})
	}
}
#[async_trait]
impl AccessTokenSource for MetadataTokenSource {
	async fn token(&self) -> Result<SecretString, SignError> {
		let mut cached = self.cached.lock().await;
		if let Some((value, expiry)) = &*cached
			&& *expiry > Instant::now()
		{
			return Ok(value.clone());
		}
		#[derive(Deserialize)]
		struct Token {
			access_token: String,
			expires_in: u64,
			token_type: String,
		}
		let response = self
			.client
			.get(&self.url)
			.header("Metadata-Flavor", "Google")
			.send()
			.await
			.map_err(|_| SignError)?;
		if response
			.headers()
			.get("Metadata-Flavor")
			.and_then(|v| v.to_str().ok())
			!= Some("Google")
		{
			return Err(SignError);
		}
		let value: Token = bounded_json(response, 16_384).await?;
		let token: SecretString = value.access_token.into();
		if token.expose_secret().is_empty()
			|| value.token_type != "Bearer"
			|| value.expires_in <= 60
		{
			return Err(SignError);
		}
		*cached = Some((
			token.clone(),
			Instant::now() + Duration::from_secs(value.expires_in - 60),
		));
		Ok(token)
	}
}

fn client() -> Result<reqwest::Client, SignError> {
	reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.connect_timeout(Duration::from_secs(3))
		.timeout(Duration::from_secs(15))
		.build()
		.map_err(|_| SignError)
}

/// Ed25519 KMS requires raw JWS input in `data`, never a precomputed digest.
pub struct KmsTokenSigner {
	kid: String,
	base: String,
	client: reqwest::Client,
	tokens: Arc<dyn AccessTokenSource>,
}
impl KmsTokenSigner {
	pub fn new(kid: String, tokens: Arc<dyn AccessTokenSource>) -> Result<Self, SignError> {
		let parts: Vec<_> = kid.split('/').collect();
		if parts.len() != 10
			|| parts[0] != "projects"
			|| parts[2] != "locations"
			|| parts[4] != "keyRings"
			|| parts[6] != "cryptoKeys"
			|| parts[8] != "cryptoKeyVersions"
			|| !parts.iter().all(|p| {
				!p.is_empty()
					&& p.bytes()
						.all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
			}) || !numeric_version(parts[9])
		{
			return Err(SignError);
		}
		Ok(Self {
			kid,
			base: "https://cloudkms.googleapis.com/v1".into(),
			client: client()?,
			tokens,
		})
	}
}
#[async_trait]
impl TokenSigner for KmsTokenSigner {
	fn kid(&self) -> &str {
		&self.kid
	}
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError> {
		#[derive(Deserialize)]
		#[serde(rename_all = "camelCase")]
		struct Signed {
			name: String,
			signature: String,
			signature_crc32c: String,
			verified_data_crc32c: bool,
		}
		let token = self.tokens.token().await?;
		let response = self
			.client
			.post(format!("{}/{}:asymmetricSign", self.base, self.kid))
			.bearer_auth(token.expose_secret())
			.json(&serde_json::json!({
				"data": STANDARD.encode(input), "dataCrc32c": crc32c::crc32c(input).to_string()
			}))
			.send()
			.await
			.map_err(|_| SignError)?;
		let signed: Signed = bounded_json(response, 16_384).await?;
		let signature = STANDARD.decode(&signed.signature).map_err(|_| SignError)?;
		if signed.name != self.kid
			|| !signed.verified_data_crc32c
			|| signed.signature_crc32c.parse::<u32>().ok() != Some(crc32c::crc32c(&signature))
		{
			return Err(SignError);
		}
		signature.try_into().map_err(|_| SignError)
	}
}

/// Production Key Material source. The broker identity alone receives versions.access.
pub struct SecretManagerKeyMaterialSource {
	resource_prefix: String,
	base: String,
	client: reqwest::Client,
	tokens: Arc<dyn AccessTokenSource>,
}
impl SecretManagerKeyMaterialSource {
	pub fn new(
		project_number: &str,
		secret_prefix: &str,
		tokens: Arc<dyn AccessTokenSource>,
	) -> Result<Self, SignError> {
		if project_number.is_empty()
			|| !project_number.bytes().all(|b| b.is_ascii_digit())
			|| !secret_prefix.starts_with("aidash-")
			|| !secret_prefix.ends_with("-cred-")
			|| !secret_prefix
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
		{
			return Err(SignError);
		}
		Ok(Self {
			resource_prefix: format!("projects/{project_number}/secrets/{secret_prefix}"),
			base: "https://secretmanager.googleapis.com/v1".into(),
			client: client()?,
			tokens,
		})
	}
}
#[async_trait]
impl KeyMaterialSource for SecretManagerKeyMaterialSource {
	async fn access(&self, secret: &str, version: &str) -> Result<SecretString, KeyMaterialError> {
		let id = secret
			.strip_prefix(&self.resource_prefix)
			.and_then(|id| uuid::Uuid::parse_str(id).ok());
		if id.is_none_or(|id| id.is_nil()) || !numeric_version(version) {
			return Err(KeyMaterialError::Unavailable);
		}
		let token = self
			.tokens
			.token()
			.await
			.map_err(|_| KeyMaterialError::StoreUnavailable)?;
		let response = self
			.client
			.get(format!("{}/{secret}/versions/{version}:access", self.base))
			.bearer_auth(token.expose_secret())
			.send()
			.await
			.map_err(|_| KeyMaterialError::StoreUnavailable)?;
		if matches!(response.status().as_u16(), 403 | 404 | 409) {
			return Err(KeyMaterialError::Unavailable);
		}
		#[derive(Deserialize)]
		#[serde(rename_all = "camelCase")]
		struct Payload {
			data: String,
			data_crc32c: String,
		}
		#[derive(Deserialize)]
		struct Accessed {
			name: String,
			payload: Payload,
		}
		let accessed: Accessed = bounded_json(response, 65_536)
			.await
			.map_err(|_| KeyMaterialError::StoreUnavailable)?;
		let data = Zeroizing::new(accessed.payload.data);
		let bytes = Zeroizing::new(
			STANDARD
				.decode(data.as_bytes())
				.map_err(|_| KeyMaterialError::StoreUnavailable)?,
		);
		if accessed.name != format!("{secret}/versions/{version}")
			|| accessed.payload.data_crc32c.parse::<u32>().ok() != Some(crc32c::crc32c(&bytes))
			|| bytes.is_empty()
			|| bytes.len() > 16_384
		{
			return Err(KeyMaterialError::StoreUnavailable);
		}
		let text = std::str::from_utf8(&bytes).map_err(|_| KeyMaterialError::StoreUnavailable)?;
		if text.contains(['\r', '\n']) {
			return Err(KeyMaterialError::Unavailable);
		}
		Ok(SecretString::from(text.to_owned()))
	}
}
fn numeric_version(value: &str) -> bool {
	!value.is_empty() && !value.starts_with('0') && value.bytes().all(|b| b.is_ascii_digit())
}
async fn bounded_json<T: DeserializeOwned>(
	mut response: reqwest::Response,
	limit: usize,
) -> Result<T, SignError> {
	if !response.status().is_success()
		|| response.content_length().is_some_and(|n| n > limit as u64)
	{
		return Err(SignError);
	}
	let mut body = Zeroizing::new(Vec::new());
	while let Some(chunk) = response.chunk().await.map_err(|_| SignError)? {
		if body.len().saturating_add(chunk.len()) > limit {
			return Err(SignError);
		}
		body.extend_from_slice(&chunk);
	}
	serde_json::from_slice(&body).map_err(|_| SignError)
}

#[cfg(test)]
mod tests;
