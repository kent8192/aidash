//! One project registration for browser routes and typed server functions.
//!
//! Only the WASM client discovers routes through `inventory`; the native
//! binaries build their handler from `handler()` directly. Reinhardt allows one
//! `#[routes]` marker per link unit, and Auth's cross-service tests link the
//! Marketplace library next to Auth's own registration, so a native marker in
//! either library would collide with the other at link time.
use reinhardt::UnifiedRouter;

#[cfg_attr(client, reinhardt::routes)]
pub fn routes() -> UnifiedRouter {
	UnifiedRouter::new().merge(crate::apps::authentication::urls::url_patterns())
}
#[cfg(server)]
mod native {
	use super::routes;
	use crate::apps::authentication::server::rpc::Browser;
	use reinhardt::di::{InjectionContext, SingletonScope};
	use reinhardt::{Handler, Request, Response, ServerRouter};
	use std::sync::Arc;
	pub struct Application {
		protocol: crate::http::Http,
		rpc: ServerRouter,
		hsts: bool,
	}
	pub fn handler(service: Arc<crate::service::Authentication>) -> Application {
		let singleton = Arc::new(SingletonScope::new());
		singleton.set(Browser(service.clone()));
		let context = Arc::new(InjectionContext::builder(singleton).build());
		Application {
			hsts: !service.config.loopback_development,
			protocol: crate::http::Http::new(service.clone()),
			rpc: routes().with_di_context(context).into_server(),
		}
	}
	#[async_trait::async_trait]
	impl Handler for Application {
		async fn handle(&self, request: Request) -> reinhardt::core::exception::Result<Response> {
			if request.uri.path().starts_with("/api/server_fn/") {
				let response = self.rpc.handle(request).await?;
				let response = if self.hsts {
					response.with_header("strict-transport-security", crate::http::HSTS)
				} else {
					response
				};
				return Ok(response
					.with_header("cache-control", "no-store")
					.with_header("x-content-type-options", "nosniff")
					.with_header("referrer-policy", "same-origin")
					.with_header(
						"content-security-policy",
						"default-src 'none'; frame-ancestors 'none'",
					));
			}
			self.protocol.handle(request).await
		}
	}
}
#[cfg(server)]
pub use native::{Application, handler};
