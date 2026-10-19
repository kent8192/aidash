//! Cached JSON Web Key Sets of token issuers.
use crate::{Error, Result};
use async_trait::async_trait;
use jsonwebtoken::{DecodingKey, jwk::JwkSet};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

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
			.map_err(|_| Error::External("signing keys unavailable".into()))?;
		if !response.status().is_success() {
			return Err(Error::External("signing keys unavailable".into()));
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
			.map_err(|_| Error::External("signing keys unavailable".into()))?;
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
		.map_err(|_| Error::External("signing keys unavailable".into()))?
	{
		if bytes.len() + chunk.len() > 1_048_576 {
			return Err(Error::External("signing keys response too large".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

#[cfg(test)]
mod tests;
