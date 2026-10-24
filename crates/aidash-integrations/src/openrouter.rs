//! Credential-free, bounded access to the public OpenRouter catalog.
use aidash_application::{Result, ports::ModelCatalog};
use aidash_domain::catalog::{CatalogModel, eligible};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

pub struct OpenRouterCatalog {
	client: reqwest::Client,
}

impl OpenRouterCatalog {
	pub fn new(client: reqwest::Client) -> Self {
		Self { client }
	}
}

#[derive(Deserialize)]
struct Catalog {
	data: Vec<CatalogModel>,
}

#[async_trait]
impl ModelCatalog for OpenRouterCatalog {
	async fn models(&self) -> Result<Vec<CatalogModel>> {
		let response = self
			.client
			.get("https://openrouter.ai/api/v1/models")
			.timeout(Duration::from_secs(15))
			.send()
			.await
			.map_err(crate::http_error)?
			.error_for_status()
			.map_err(crate::http_error)?;
		let catalog: Catalog = crate::response::json(response, 8 * 1024 * 1024).await?;
		Ok(eligible(catalog.data))
	}
}
