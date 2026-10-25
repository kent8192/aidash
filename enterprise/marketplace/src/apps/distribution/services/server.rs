use crate::{Config, migrations, store};
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
	pub(crate) fn response(&self) -> Response {
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
/// Two years; sent whenever the configured origin is HTTPS.
const HSTS: &str = "max-age=63072000; includeSubDomains";
pub struct Marketplace {
	pub(crate) config: Config,
	pub(crate) store: store::Store,
	pub(crate) http: reqwest::Client,
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
	pub(crate) fn strict_transport(&self, response: Response) -> Response {
		if self.config.secure() {
			response.with_header("strict-transport-security", HSTS)
		} else {
			response
		}
	}
	/// Refuses plain-HTTP requests when the origin is HTTPS. Forwarded HTTPS
	/// metadata counts only from the configured proxy IPs or networks; probes stay reachable.
	///
	/// Every `/internal/` route is exempt (any method). These are machine-to-machine,
	/// Bearer-authenticated, never carry browser cookies, and the public edge denies
	/// them, so they keep their in-cluster plain-HTTP path. The chart's NetworkPolicy
	/// limits who can reach it. `/api/marketplace/downloads` and `/recalls` stay
	/// enforced, because remote Registry Connections reach them through the HTTPS edge.
	pub(crate) fn require_https(&self, request: &Request) -> Result<()> {
		let path = request.uri.path();
		if !self.config.secure()
			|| (request.method == http::Method::GET && path == "/healthz")
			|| path.starts_with("/internal/")
		{
			return Ok(());
		}
		request.set_trusted_proxies(reinhardt::http::TrustedProxies::new(
			crate::config::settings::trusted_peer(
				&self.config.trusted_proxy_ips,
				request.remote_addr,
			),
		));
		if request.is_secure() {
			Ok(())
		} else {
			Err(Error::Invalid("HTTPS is required"))
		}
	}
}
#[derive(Clone)]
pub struct MarketplaceHandler(Arc<Marketplace>);
#[async_trait::async_trait]
impl Handler for MarketplaceHandler {
	async fn handle(&self, request: Request) -> reinhardt::core::exception::Result<Response> {
		let pages = request.uri.path() == "/" || request.uri.path().starts_with("/assets/");
		let result = match self.0.require_https(&request) {
			Ok(()) => self.0.dispatch(request).await,
			Err(error) => Err(error),
		};
		let response = match result {
			Ok(response) => response,
			Err(error) => error.response(),
		};
		Ok(self
			.0
			.strict_transport(response)
			.with_header("x-content-type-options", "nosniff")
			.with_header("referrer-policy", "same-origin")
			.with_header(
				"content-security-policy",
				if pages { "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; font-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'" } else { "default-src 'none'; frame-ancestors 'none'" },
			))
	}
}
pub(crate) fn json(value: impl serde::Serialize) -> Result<Response> {
	Ok(
		Response::from_json_body(StatusCode::OK, serde_json::to_vec(&value)?)
			.with_header("cache-control", "no-store"),
	)
}
