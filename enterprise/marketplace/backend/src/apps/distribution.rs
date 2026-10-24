//! Independently deployable Marketplace HTTP and persistence adapters.
pub(crate) mod auth;
pub use crate::config::settings as config;
pub mod migrations;
pub(crate) mod store;
pub(crate) mod views;
pub use config::Config;
use http::StatusCode;
use reinhardt::http::{Handler, Request, Response};
use std::sync::Arc;
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("{0}")]
	Invalid(&'static str),
	#[error("authentication required")]
	Unauthorized,
	#[error("operation is not authorized")]
	Forbidden,
	#[error("resource not found")]
	NotFound,
	#[error("resource already exists")]
	Conflict,
	#[error("dependency unavailable")]
	Unavailable,
	#[error("persistence failure")]
	Persistence(#[from] reinhardt::core::exception::Error),
	#[error("invalid JSON")]
	Json(#[from] serde_json::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
	fn response(&self) -> Response {
		let status = match self {
			Self::Invalid(_) | Self::Json(_) => StatusCode::BAD_REQUEST,
			Self::Unauthorized => StatusCode::UNAUTHORIZED,
			Self::Forbidden => StatusCode::FORBIDDEN,
			Self::NotFound => StatusCode::NOT_FOUND,
			Self::Conflict => StatusCode::CONFLICT,
			Self::Unavailable | Self::Persistence(_) => StatusCode::SERVICE_UNAVAILABLE,
		};
		Response::from_json_body(
			status,
			serde_json::to_vec(&serde_json::json!({"error":self.to_string()})).unwrap(),
		)
		.with_header("cache-control", "no-store")
	}
}
pub struct Marketplace {
	config: Config,
	store: store::Store,
	http: reqwest::Client,
}
impl Marketplace {
	pub async fn connect(config: Config, database_url: &str) -> Result<Arc<Self>> {
		migrations::require_current(database_url).await?;
		let store = store::Store::connect(database_url).await?;
		store.require_runtime_role().await?;
		let http = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.timeout(std::time::Duration::from_secs(10))
			.build()
			.map_err(|_| Error::Unavailable)?;
		Ok(Arc::new(Self {
			config,
			store,
			http,
		}))
	}
	pub fn handler(self: &Arc<Self>) -> MarketplaceHandler {
		MarketplaceHandler(self.clone())
	}
}
#[derive(Clone)]
pub struct MarketplaceHandler(Arc<Marketplace>);
#[async_trait::async_trait]
impl Handler for MarketplaceHandler {
	async fn handle(&self, request: Request) -> reinhardt::core::exception::Result<Response> {
		let response = match self.0.dispatch(request).await {
			Ok(response) => response,
			Err(error) => error.response(),
		};
		Ok(response
			.with_header("x-content-type-options", "nosniff")
			.with_header("referrer-policy", "same-origin")
			.with_header(
				"content-security-policy",
				"default-src 'none'; frame-ancestors 'none'",
			))
	}
}
pub(crate) fn json(value: impl serde::Serialize) -> Result<Response> {
	Ok(
		Response::from_json_body(StatusCode::OK, serde_json::to_vec(&value)?)
			.with_header("cache-control", "no-store"),
	)
}

#[cfg(test)]
mod tests;

pub mod urls;
#[reinhardt::app_config(name = "distribution", label = "marketplace_distribution")]
pub struct DistributionConfig;
