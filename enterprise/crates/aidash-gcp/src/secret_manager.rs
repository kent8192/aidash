//! Write-only Secret Manager REST adapter of the Secret Store.
use aidash_application::{Error, Result, provider_credentials::Store};
use async_trait::async_trait;
use base64::Engine;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

pub struct SecretManager {
	client: reqwest::Client,
	/// Metadata tokens carry Store authority and must never traverse an HTTP proxy.
	metadata_client: reqwest::Client,
	project: String,
	prefix: String,
	api: String,
	metadata: String,
}
impl SecretManager {
	pub fn new(project: String, environment: String) -> Result<Self> {
		if project.is_empty()
			|| !project
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
			|| environment.is_empty()
			|| !environment
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		{
			return Err(Error::Invalid(
				"invalid Secret Store project or environment".into(),
			));
		}
		let client = |builder: reqwest::ClientBuilder| {
			builder
				.timeout(Duration::from_secs(20))
				.connect_timeout(Duration::from_secs(3))
				.redirect(reqwest::redirect::Policy::none())
				.build()
				.map_err(aidash_integrations::http_error)
		};
		Ok(Self {
			client: client(reqwest::Client::builder())?,
			metadata_client: client(reqwest::Client::builder().no_proxy())?,
			project,
			prefix: format!("aidash-{environment}-cred-"),
			api: "https://secretmanager.googleapis.com/v1".into(),
			metadata:
				"http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token"
					.into(),
		})
	}
	fn validate_resource(&self, resource: &str) -> Result<()> {
		let prefix = format!("projects/{}/secrets/{}", self.project, self.prefix);
		let id = resource.strip_prefix(&prefix).ok_or_else(|| {
			Error::Invalid("Secret Store resource is outside its environment".into())
		})?;
		let id = Uuid::parse_str(id)
			.map_err(|_| Error::Invalid("invalid Secret Store resource".into()))?;
		if id.get_version_num() != 7 {
			return Err(Error::Invalid(
				"Provider Credential ID must be UUIDv7".into(),
			));
		}
		Ok(())
	}
	fn validate_version(&self, version: &str) -> Result<()> {
		let (resource, id) = version
			.rsplit_once("/versions/")
			.ok_or_else(|| Error::Invalid("invalid Secret Store version".into()))?;
		self.validate_resource(resource)?;
		if id.parse::<u64>().ok().filter(|n| *n > 0).is_none() {
			return Err(Error::Invalid(
				"Secret Store requires a numeric version pin".into(),
			));
		}
		Ok(())
	}
	async fn token(&self) -> Result<SecretString> {
		#[derive(Deserialize)]
		struct Token {
			access_token: SecretString,
		}
		let response = self
			.metadata_client
			.get(&self.metadata)
			.header("Metadata-Flavor", "Google")
			.send()
			.await
			.map_err(aidash_integrations::http_error)?;
		if !response.status().is_success()
			|| response
				.headers()
				.get("Metadata-Flavor")
				.and_then(|v| v.to_str().ok())
				!= Some("Google")
		{
			return Err(Error::External("Secret Store token is unavailable".into()));
		}
		let token: Token = aidash_integrations::response::json(response, 16384).await?;
		Ok(token.access_token)
	}
	async fn call(
		&self,
		method: reqwest::Method,
		path: &str,
		body: Option<Value>,
		missing_ok: bool,
	) -> Result<Option<Value>> {
		let token = self.token().await?;
		let mut request = self
			.client
			.request(method, format!("{}/{}", self.api, path))
			.bearer_auth(token.expose_secret());
		if let Some(body) = body {
			request = request.json(&body);
		}
		let response = request
			.send()
			.await
			.map_err(aidash_integrations::http_error)?;
		if response.status() == reqwest::StatusCode::NOT_FOUND && missing_ok {
			return Ok(None);
		}
		if !response.status().is_success() {
			return Err(Error::External(format!(
				"Secret Store request failed ({})",
				response.status().as_u16()
			)));
		}
		if response.status() == reqwest::StatusCode::NO_CONTENT {
			return Ok(None);
		}
		Ok(Some(
			aidash_integrations::response::json(response, 1024 * 1024)
				.await
				.map_err(|_| Error::External("Secret Store response is unavailable".into()))?,
		))
	}
}
#[async_trait]
impl Store for SecretManager {
	fn resource(&self, id: Uuid) -> String {
		format!("projects/{}/secrets/{}{id}", self.project, self.prefix)
	}
	async fn create(&self, _tenant: &str, id: Uuid) -> Result<()> {
		self.validate_resource(&self.resource(id))?;
		let environment = self
			.prefix
			.strip_prefix("aidash-")
			.unwrap()
			.strip_suffix("-cred-")
			.unwrap();
		self.call(reqwest::Method::POST,&format!("projects/{}/secrets?secretId={}{id}",self.project,self.prefix),Some(json!({"replication":{"userManaged":{"replicas":[{"location":"us-central1"}]}},"labels":{"environment":environment,"credential-id":id.to_string()}})),false).await?;
		Ok(())
	}
	async fn add_version(
		&self,
		_tenant: &str,
		resource: &str,
		key: &SecretString,
	) -> Result<String> {
		self.validate_resource(resource)?;
		let data = base64::engine::general_purpose::STANDARD.encode(key.expose_secret().as_bytes());
		let response = self
			.call(
				reqwest::Method::POST,
				&format!("{resource}:addVersion"),
				Some(json!({"payload":{"data":data}})),
				false,
			)
			.await?
			.ok_or_else(|| Error::External("Secret Store returned no version".into()))?;
		let name = response["name"]
			.as_str()
			.ok_or_else(|| Error::External("Secret Store returned no version".into()))?;
		let id = name.rsplit('/').next().unwrap_or_default();
		let version = format!("{resource}/versions/{id}");
		self.validate_version(&version)?;
		Ok(version)
	}
	async fn versions(&self, _tenant: &str, resource: &str) -> Result<Vec<String>> {
		self.validate_resource(resource)?;
		let mut versions = Vec::new();
		let mut page = String::new();
		loop {
			let mut path = format!("{resource}/versions?pageSize=100");
			if !page.is_empty() {
				path.push_str("&pageToken=");
				path.push_str(
					&percent_encoding::utf8_percent_encode(
						&page,
						percent_encoding::NON_ALPHANUMERIC,
					)
					.to_string(),
				);
			}
			let Some(value) = self.call(reqwest::Method::GET, &path, None, true).await? else {
				return Ok(versions);
			};
			if let Some(items) = value["versions"].as_array() {
				for v in items {
					if v["state"] == "DESTROYED" {
						continue;
					}
					let name = v["name"].as_str().ok_or_else(|| {
						Error::External("Secret Store returned an invalid version".into())
					})?;
					let version = format!(
						"{resource}/versions/{}",
						name.rsplit('/').next().unwrap_or_default()
					);
					self.validate_version(&version)?;
					versions.push(version);
				}
			}
			page = value["nextPageToken"].as_str().unwrap_or_default().into();
			if page.is_empty() {
				break;
			}
			if versions.len() > 10000 {
				return Err(Error::External(
					"Secret Store version inventory exceeds its limit".into(),
				));
			}
		}
		Ok(versions)
	}
	async fn disable(&self, _tenant: &str, version: &str) -> Result<()> {
		self.validate_version(version)?;
		let Some(metadata) = self.call(reqwest::Method::GET, version, None, true).await? else {
			return Ok(());
		};
		if metadata["state"] == "DISABLED" || metadata["state"] == "DESTROYED" {
			return Ok(());
		}
		self.call(
			reqwest::Method::POST,
			&format!("{version}:disable"),
			Some(json!({})),
			true,
		)
		.await?;
		Ok(())
	}
	async fn destroy(&self, _tenant: &str, version: &str) -> Result<()> {
		self.validate_version(version)?;
		let Some(metadata) = self.call(reqwest::Method::GET, version, None, true).await? else {
			return Ok(());
		};
		if metadata["state"] == "DESTROYED" {
			return Ok(());
		}
		self.call(
			reqwest::Method::POST,
			&format!("{version}:destroy"),
			Some(json!({})),
			true,
		)
		.await?;
		Ok(())
	}
	async fn delete(&self, _tenant: &str, resource: &str) -> Result<()> {
		self.validate_resource(resource)?;
		self.call(reqwest::Method::DELETE, resource, None, true)
			.await?;
		Ok(())
	}
}
#[cfg(test)]
mod tests;
