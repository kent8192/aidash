//! Independently durable recovery objects stored through the GCS JSON API.
use crate::{Error, Result};
use aidash_application::ports::memory::objects::{
	MAX_OBJECT_BYTES, Object, ObjectName, ObjectPage, RecoveryObjects,
};
use async_trait::async_trait;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct GcsObjects {
	client: Client,
	bucket: String,
	endpoint: String,
	metadata: String,
	token: Mutex<Option<(SecretString, Instant)>>,
}
#[derive(Deserialize)]
struct Token {
	access_token: String,
	expires_in: u64,
}
#[derive(Deserialize)]
struct Metadata {
	generation: String,
}
#[derive(Deserialize)]
struct Listed {
	name: String,
	generation: String,
}
#[derive(Deserialize)]
struct Listing {
	#[serde(default)]
	items: Vec<Listed>,
	#[serde(rename = "nextPageToken")]
	next: Option<String>,
}

impl GcsObjects {
	pub fn new(bucket: String) -> Result<Self> {
		Self::at(bucket, "https://storage.googleapis.com".into(),
			"http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token".into())
	}
	fn at(bucket: String, endpoint: String, metadata: String) -> Result<Self> {
		if !(3..=222).contains(&bucket.len())
			|| !bucket.bytes().all(|byte| {
				byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
			}) {
			return Err(Error::Invalid("invalid recovery bucket name".into()));
		}
		crate::validate_endpoint(&endpoint)?;
		let client = Client::builder()
			.no_proxy()
			.redirect(reqwest::redirect::Policy::none())
			.timeout(Duration::from_secs(20))
			.connect_timeout(Duration::from_secs(5))
			.build()
			.map_err(|_| unavailable())?;
		Ok(Self {
			client,
			bucket,
			endpoint: endpoint.trim_end_matches('/').into(),
			metadata,
			token: Mutex::new(None),
		})
	}
	async fn credential(&self) -> Result<SecretString> {
		let mut cached = self.token.lock().await;
		if let Some((token, expires)) = &*cached
			&& *expires > Instant::now()
		{
			return Ok(token.clone());
		}
		let response = self
			.client
			.get(&self.metadata)
			.header("Metadata-Flavor", "Google")
			.timeout(Duration::from_secs(5))
			.send()
			.await
			.map_err(|_| unavailable())?;
		if !response.status().is_success() {
			return Err(unavailable());
		}
		let token: Token = serde_json::from_slice(&bounded(response, 64 * 1024).await?)
			.map_err(|_| unavailable())?;
		if token.access_token.is_empty()
			|| token.access_token.len() > 16384
			|| token.expires_in == 0
		{
			return Err(unavailable());
		}
		let value: SecretString = token.access_token.into();
		let expires = Instant::now()
			.checked_add(Duration::from_secs(token.expires_in.saturating_sub(30)))
			.ok_or_else(unavailable)?;
		*cached = Some((value.clone(), expires));
		Ok(value)
	}
	async fn request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
		let credential = self.credential().await?;
		Ok(self
			.client
			.request(method, format!("{}{path}", self.endpoint))
			.bearer_auth(credential.expose_secret()))
	}
	fn object_path(&self, name: &str) -> Result<String> {
		validate_name(name)?;
		Ok(format!(
			"/storage/v1/b/{}/o/{}",
			self.bucket,
			utf8_percent_encode(name, NON_ALPHANUMERIC)
		))
	}
}

#[async_trait]
impl RecoveryObjects for GcsObjects {
	async fn get(&self, name: &str, generation: Option<&str>) -> Result<Option<Object>> {
		let path = self.object_path(name)?;
		let generation = match generation {
			Some(value) => {
				validate_generation(value)?;
				value.to_owned()
			}
			None => {
				let response = self
					.request(Method::GET, &path)
					.await?
					.query(&[("fields", "generation")])
					.send()
					.await
					.map_err(|_| unavailable())?;
				if response.status() == StatusCode::NOT_FOUND {
					return Ok(None);
				}
				check_status("read metadata", response.status())?;
				let metadata: Metadata =
					serde_json::from_slice(&bounded(response, 64 * 1024).await?)
						.map_err(|_| unavailable())?;
				validate_generation(&metadata.generation)?;
				metadata.generation
			}
		};
		let response = self
			.request(Method::GET, &path)
			.await?
			.query(&[("alt", "media"), ("generation", generation.as_str())])
			.send()
			.await
			.map_err(|_| unavailable())?;
		if response.status() == StatusCode::NOT_FOUND {
			return Ok(None);
		}
		check_status("read object", response.status())?;
		Ok(Some(Object {
			bytes: bounded(response, MAX_OBJECT_BYTES).await?,
			generation,
		}))
	}
	async fn create(&self, name: &str, bytes: &[u8]) -> Result<String> {
		validate_name(name)?;
		if bytes.len() > MAX_OBJECT_BYTES {
			return Err(Error::Invalid(
				"recovery object exceeds its size cap".into(),
			));
		}
		let response = self
			.request(
				Method::POST,
				&format!("/upload/storage/v1/b/{}/o", self.bucket),
			)
			.await?
			.query(&[
				("uploadType", "media"),
				("name", name),
				("ifGenerationMatch", "0"),
			])
			.header("content-type", "application/octet-stream")
			.body(bytes.to_owned())
			.send()
			.await
			.map_err(|_| unavailable())?;
		if response.status() == StatusCode::PRECONDITION_FAILED {
			let existing = self.get(name, None).await?.ok_or_else(unavailable)?;
			if existing.bytes != bytes {
				return Err(Error::Conflict(
					"immutable recovery object already has different contents".into(),
				));
			}
			return Ok(existing.generation);
		}
		check_status("create object", response.status())?;
		let metadata: Metadata = serde_json::from_slice(&bounded(response, 64 * 1024).await?)
			.map_err(|_| unavailable())?;
		validate_generation(&metadata.generation)?;
		Ok(metadata.generation)
	}
	async fn list(&self, prefix: &str, page: Option<&str>) -> Result<ObjectPage> {
		if prefix.len() > 1024 || prefix.chars().any(char::is_control) {
			return Err(Error::Invalid("invalid recovery object prefix".into()));
		}
		let mut request = self
			.request(Method::GET, &format!("/storage/v1/b/{}/o", self.bucket))
			.await?
			.query(&[
				("prefix", prefix),
				("maxResults", "1000"),
				("fields", "items(name,generation),nextPageToken"),
			]);
		if let Some(page) = page {
			request = request.query(&[("pageToken", page)]);
		}
		let response = request.send().await.map_err(|_| unavailable())?;
		check_status("list objects", response.status())?;
		let listing: Listing = serde_json::from_slice(&bounded(response, 1024 * 1024).await?)
			.map_err(|_| unavailable())?;
		let mut objects = Vec::with_capacity(listing.items.len());
		for item in listing.items {
			validate_name(&item.name)?;
			validate_generation(&item.generation)?;
			if !item.name.starts_with(prefix) {
				return Err(Error::Invalid("recovery listing escaped its prefix".into()));
			}
			objects.push(ObjectName {
				name: item.name,
				generation: item.generation,
			});
		}
		Ok(ObjectPage {
			objects,
			next: listing.next.filter(|next| !next.is_empty()),
		})
	}
	async fn delete(&self, name: &str, generation: &str) -> Result<()> {
		validate_generation(generation)?;
		let response = self
			.request(Method::DELETE, &self.object_path(name)?)
			.await?
			.query(&[("ifGenerationMatch", generation)])
			.send()
			.await
			.map_err(|_| unavailable())?;
		if response.status() == StatusCode::NOT_FOUND {
			return Ok(());
		}
		check_status("delete object", response.status())
	}
}

fn unavailable() -> Error {
	Error::External("recovery object storage is unavailable".into())
}
fn check_status(action: &str, status: StatusCode) -> Result<()> {
	if status.is_success() {
		Ok(())
	} else {
		// Never include provider response bodies, request URLs or bearer tokens.
		Err(Error::External(format!(
			"recovery storage could not {action} (HTTP {})",
			status.as_u16()
		)))
	}
}
fn validate_generation(value: &str) -> Result<()> {
	if value.is_empty() || value.len() > 30 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
		return Err(Error::Invalid("invalid recovery object generation".into()));
	}
	Ok(())
}
fn validate_name(value: &str) -> Result<()> {
	if value.is_empty()
		|| value.len() > 1024
		|| value.chars().any(char::is_control)
		|| value
			.split('/')
			.any(|part| part.is_empty() || matches!(part, "." | ".."))
	{
		return Err(Error::Invalid("invalid recovery object name".into()));
	}
	Ok(())
}
async fn bounded(mut response: Response, limit: usize) -> Result<Vec<u8>> {
	if response
		.content_length()
		.is_some_and(|length| length > limit as u64)
	{
		return Err(Error::Invalid(
			"recovery object exceeds its size cap".into(),
		));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
		if bytes.len().saturating_add(chunk.len()) > limit {
			return Err(Error::Invalid(
				"recovery object exceeds its size cap".into(),
			));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}

#[cfg(test)]
mod tests;
