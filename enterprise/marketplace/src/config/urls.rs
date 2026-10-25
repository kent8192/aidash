//! One project registration for browser routes and typed server functions.
//!
//! Only the WASM client discovers routes through `inventory`; the native
//! binaries build their handler from `handler()` directly. Reinhardt allows one
//! `#[routes]` marker per link unit, and Auth's cross-service tests link this
//! library next to Auth's own registration, so a native marker here would
//! collide with Auth's marker at link time.
use reinhardt::UnifiedRouter;

#[cfg_attr(client, reinhardt::routes)]
pub fn routes() -> UnifiedRouter {
	UnifiedRouter::new().merge(crate::apps::distribution::urls::url_patterns())
}
#[cfg(server)]
mod native {
	use super::routes;
	use crate::apps::distribution::server::rpc::Browser;
	use reinhardt::di::{InjectionContext, SingletonScope};
	use reinhardt::{Handler, Request, Response, ServerRouter};
	use std::sync::Arc;
	pub struct Application {
		service: Arc<crate::Marketplace>,
		protocol: crate::MarketplaceHandler,
		rpc: ServerRouter,
	}
	pub fn handler(service: Arc<crate::Marketplace>) -> Application {
		let singleton = Arc::new(SingletonScope::new());
		singleton.set(Browser(service.clone()));
		let context = Arc::new(InjectionContext::builder(singleton).build());
		Application {
			protocol: service.handler(),
			service,
			rpc: routes().with_di_context(context).into_server(),
		}
	}
	#[async_trait::async_trait]
	impl Handler for Application {
		async fn handle(&self, request: Request) -> reinhardt::core::exception::Result<Response> {
			if request.uri.path().starts_with("/api/server_fn/") {
				if let Err(error) = self.service.require_https(&request) {
					return Ok(self.service.strict_transport(error.response()));
				}
				let response = self
					.service
					.strict_transport(self.rpc.handle(request).await?);
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
