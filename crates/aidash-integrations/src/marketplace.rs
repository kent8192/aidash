//! Marketplace distribution uses only the configured Deployment endpoint and
//! Connection. Neither a source URI nor a caller supplies transport credentials.
use crate::{Error, Result};
use aidash_application::ports::registry::{
	AgentPluginSource, FetchedPlugin, PluginFetch, PluginImportRequest,
};
use aidash_domain::{
	agent_plugin::PluginFile,
	marketplace::distribution::{Artifact, Recall},
};
use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;
#[derive(Clone)]
pub struct Connection {
	pub connection_id: Uuid,
	pub owner_account_id: Uuid,
	pub owner_tenant_id: Option<String>,
	pub secret_file: PathBuf,
}
#[derive(Clone)]
pub struct Client {
	endpoint: String,
	connection: Connection,
	http: reqwest::Client,
}
impl Client {
	pub fn new(endpoint: &str, connection: Connection) -> Result<Self> {
		crate::validate_endpoint(endpoint)?;
		let parsed = reqwest::Url::parse(endpoint)
			.map_err(|_| Error::Invalid("invalid Marketplace endpoint".into()))?;
		if parsed.scheme() != "https"
			&& !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
		{
			return Err(Error::Invalid(
				"Marketplace requires HTTPS except loopback".into(),
			));
		}
		let http = reqwest::Client::builder()
			.timeout(std::time::Duration::from_secs(30))
			.connect_timeout(std::time::Duration::from_secs(5))
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.map_err(crate::http_error)?;
		Ok(Self {
			endpoint: endpoint.trim_end_matches('/').into(),
			connection,
			http,
		})
	}
	async fn secret(&self) -> Result<secrecy::SecretString> {
		let bytes = tokio::fs::read(&self.connection.secret_file)
			.await
			.map_err(|_| Error::External("Marketplace Connection secret unavailable".into()))?;
		if bytes.len() > 8192 {
			return Err(Error::Invalid(
				"invalid Marketplace Connection secret".into(),
			));
		}
		let value = String::from_utf8(bytes)
			.map_err(|_| Error::Invalid("invalid Marketplace Connection secret".into()))?;
		let value = value.trim();
		if value.is_empty() || value.chars().any(char::is_whitespace) {
			return Err(Error::Invalid(
				"invalid Marketplace Connection secret".into(),
			));
		}
		Ok(value.to_owned().into())
	}
	pub async fn recalls(&self) -> Result<Vec<Recall>> {
		use secrecy::ExposeSecret;
		#[derive(serde::Deserialize)]
		struct Recalls {
			recalls: Vec<Recall>,
		}
		let secret = self.secret().await?;
		let response = self
			.http
			.get(format!("{}/api/marketplace/recalls", self.endpoint))
			.bearer_auth(secret.expose_secret())
			.send()
			.await
			.map_err(crate::http_error)?;
		let bytes = bounded(response).await?;
		let value: Recalls = serde_json::from_slice(&bytes)
			.map_err(|_| Error::External("invalid Marketplace Recall response".into()))?;
		Ok(value.recalls)
	}
}
const MAX_RESPONSE_BYTES: usize = 12 * 1024 * 1024;
async fn bounded(mut response: reqwest::Response) -> Result<Vec<u8>> {
	if matches!(response.status().as_u16(), 401 | 403) {
		return Err(Error::Forbidden);
	}
	if !response.status().is_success() {
		return Err(Error::External("Marketplace request refused".into()));
	}
	if response
		.content_length()
		.is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
	{
		return Err(Error::Invalid("Marketplace response too large".into()));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(crate::http_error)? {
		if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
			return Err(Error::Invalid("Marketplace response too large".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(bytes)
}
#[async_trait]
impl AgentPluginSource for Client {
	async fn fetch(&self, request: PluginImportRequest) -> Result<PluginFetch> {
		use aidash_domain::agent_plugin::MarketplaceSource;
		use base64::Engine;
		use secrecy::ExposeSecret;
		let source = MarketplaceSource::parse(&request.url).ok_or_else(|| {
			Error::Invalid("AGENT_PLUGIN_SOURCE: expected marketplace://UUID/semver".into())
		})?;
		if request.plugin_path.is_some() {
			return Err(Error::Invalid(
				"Marketplace artifacts select one immutable plugin".into(),
			));
		}
		#[derive(serde::Serialize)]
		struct Download<'a> {
			connection_id: Uuid,
			owner_account_id: Uuid,
			plugin_id: Uuid,
			version: &'a str,
			#[serde(skip_serializing_if = "Option::is_none")]
			owner_tenant_id: Option<&'a str>,
		}
		let secret = self.secret().await?;
		let response = self
			.http
			.post(format!("{}/api/marketplace/downloads", self.endpoint))
			.bearer_auth(secret.expose_secret())
			.json(&Download {
				connection_id: self.connection.connection_id,
				owner_account_id: self.connection.owner_account_id,
				plugin_id: source.plugin_id,
				version: &source.version,
				owner_tenant_id: self.connection.owner_tenant_id.as_deref(),
			})
			.send()
			.await
			.map_err(crate::http_error)?;
		let bytes = bounded(response).await?;
		let artifact: Artifact = serde_json::from_slice(&bytes)
			.map_err(|_| Error::Invalid("invalid Marketplace artifact response".into()))?;
		if artifact.plugin_id != source.plugin_id || artifact.version != source.version {
			return Err(Error::Invalid(
				"Marketplace artifact binding mismatch".into(),
			));
		}
		artifact
			.validate()
			.map_err(|reason| Error::Invalid(format!("AGENT_PLUGIN_INVALID: {reason}")))?;
		let files: Vec<PluginFile> = artifact
			.files
			.into_iter()
			.map(|(path, content)| {
				base64::engine::general_purpose::STANDARD
					.decode(content)
					.map(|bytes| PluginFile { path, bytes })
					.map_err(|_| Error::Invalid("invalid artifact base64".into()))
			})
			.collect::<Result<_>>()?;
		let plugin =
			aidash_domain::agent_plugin::load(&files).map_err(aidash_domain::Error::from)?;
		if plugin
			.manifest
			.version
			.as_deref()
			.is_some_and(|version| version != source.version)
		{
			return Err(Error::Invalid(
				"Marketplace manifest version mismatch".into(),
			));
		}
		Ok(PluginFetch::Fetched(FetchedPlugin {
			source: request.url,
			entitlement_expires_at: artifact.entitlement_expires_at,
			root: String::new(),
			files,
			denied: vec![],
		}))
	}
}
#[cfg(test)]
mod tests;

/// URI selects a product source, never an arbitrary network endpoint.
pub struct Sources {
	pub marketplace: Option<Client>,
}
#[async_trait]
impl AgentPluginSource for Sources {
	async fn fetch(&self, request: PluginImportRequest) -> Result<PluginFetch> {
		if request.url.starts_with("marketplace://") {
			self.marketplace
				.as_ref()
				.ok_or(Error::Forbidden)?
				.fetch(request)
				.await
		} else {
			crate::agent_plugin_import::PublicAgentPluginSource
				.fetch(request)
				.await
		}
	}
}
