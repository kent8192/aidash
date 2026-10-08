//! Write-only Secret Manager REST adapter and fixed-catalog key validation.
use aidash_application::{
	Error, Result,
	provider_credentials::{KeyValidator, Store, Validation},
};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use base64::Engine;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

pub struct SecretManager {
	client: reqwest::Client,
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
				"invalid Provider Credential Store project or environment".into(),
			));
		}
		Ok(Self{client:reqwest::Client::builder().timeout(Duration::from_secs(20)).connect_timeout(Duration::from_secs(3)).redirect(reqwest::redirect::Policy::none()).build().map_err(crate::http_error)?,project,prefix:format!("aidash-{environment}-cred-"),api:"https://secretmanager.googleapis.com/v1".into(),metadata:"http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token".into()})
	}
	fn validate_resource(&self, resource: &str) -> Result<()> {
		let prefix = format!("projects/{}/secrets/{}", self.project, self.prefix);
		let id = resource.strip_prefix(&prefix).ok_or_else(|| {
			Error::Invalid("Provider Credential Store resource is outside its environment".into())
		})?;
		let id = Uuid::parse_str(id)
			.map_err(|_| Error::Invalid("invalid Provider Credential Store resource".into()))?;
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
			.ok_or_else(|| Error::Invalid("invalid Provider Credential Store version".into()))?;
		self.validate_resource(resource)?;
		if id.parse::<u64>().ok().filter(|n| *n > 0).is_none() {
			return Err(Error::Invalid(
				"Provider Credential Store requires a numeric version pin".into(),
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
			.client
			.get(&self.metadata)
			.header("Metadata-Flavor", "Google")
			.send()
			.await
			.map_err(crate::http_error)?;
		if !response.status().is_success() {
			return Err(Error::External(
				"Provider Credential Store token is unavailable".into(),
			));
		}
		let token: Token = crate::response::json(response, 16384).await?;
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
		let response = request.send().await.map_err(crate::http_error)?;
		if response.status() == reqwest::StatusCode::NOT_FOUND && missing_ok {
			return Ok(None);
		}
		if !response.status().is_success() {
			return Err(Error::External(format!(
				"Provider Credential Store request failed ({})",
				response.status().as_u16()
			)));
		}
		if response.status() == reqwest::StatusCode::NO_CONTENT {
			return Ok(None);
		}
		Ok(Some(
			crate::response::json(response, 1024 * 1024)
				.await
				.map_err(|_| {
					Error::External("Provider Credential Store response is unavailable".into())
				})?,
		))
	}
}
#[async_trait]
impl Store for SecretManager {
	fn resource(&self, id: Uuid) -> String {
		format!("projects/{}/secrets/{}{id}", self.project, self.prefix)
	}
	async fn create(&self, id: Uuid) -> Result<()> {
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
	async fn add_version(&self, resource: &str, key: &SecretString) -> Result<String> {
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
			.ok_or_else(|| {
				Error::External("Provider Credential Store returned no version".into())
			})?;
		let name = response["name"].as_str().ok_or_else(|| {
			Error::External("Provider Credential Store returned no version".into())
		})?;
		let id = name.rsplit('/').next().unwrap_or_default();
		let version = format!("{resource}/versions/{id}");
		self.validate_version(&version)?;
		Ok(version)
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
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
						Error::External(
							"Provider Credential Store returned an invalid version".into(),
						)
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
					"Provider Credential Store version inventory exceeds its limit".into(),
				));
			}
		}
		Ok(versions)
	}
	async fn disable(&self, version: &str) -> Result<()> {
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
	async fn destroy(&self, version: &str) -> Result<()> {
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
	async fn delete(&self, resource: &str) -> Result<()> {
		self.validate_resource(resource)?;
		self.call(reqwest::Method::DELETE, resource, None, true)
			.await?;
		Ok(())
	}
}
pub struct OpenRouterKeyValidator {
	pub client: reqwest::Client,
}
#[async_trait]
impl KeyValidator for OpenRouterKeyValidator {
	async fn validate(&self, provider: Provider, key: &SecretString) -> Result<Validation> {
		self.validate_at(&format!("{}/key", provider.base_url()), key)
			.await
	}
}
impl OpenRouterKeyValidator {
	async fn validate_at(&self, url: &str, key: &SecretString) -> Result<Validation> {
		let response = self
			.client
			.get(url)
			.timeout(Duration::from_secs(15))
			.bearer_auth(key.expose_secret())
			.send()
			.await
			.map_err(|_| Error::ProviderRejected {
				status: 503,
				reason: "Provider Credential validation is temporarily unavailable".into(),
			})?;
		match response.status().as_u16() {
			401 | 403 => {
				return Err(Error::Invalid(
					"invalid Provider Credential Key Material".into(),
				));
			}
			429 | 500..=599 => {
				return Err(Error::ProviderRejected {
					status: 503,
					reason: "Provider Credential validation is temporarily unavailable".into(),
				});
			}
			200 => {}
			_ => {
				return Err(Error::Invalid(
					"Provider Credential validation was rejected".into(),
				));
			}
		}
		let value: Value =
			crate::response::json(response, 16384)
				.await
				.map_err(|_| Error::ProviderRejected {
					status: 503,
					reason: "Provider Credential validation response is unavailable".into(),
				})?;
		let data = value.get("data").filter(|d| d.is_object()).ok_or_else(|| {
			Error::Invalid("invalid Provider Credential validation response".into())
		})?;
		let limit = data.get("limit").ok_or_else(|| {
			Error::Invalid("Provider Credential validation omitted spending limit".into())
		})?;
		if !limit.is_null() && !limit.is_number() {
			return Err(Error::Invalid(
				"invalid Provider Credential spending limit".into(),
			));
		}
		Ok(Validation {
			warnings: if limit.is_null() {
				vec!["No spending limit is configured for this Provider Credential".into()]
			} else {
				vec![]
			},
		})
	}
}
#[cfg(test)]
mod tests;
