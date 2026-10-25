//! App-owned native and Pages route aggregation.
#[cfg(client)]
pub mod client_router;
#[cfg(server)]
pub mod server_router;
use reinhardt::UnifiedRouter;

pub fn url_patterns() -> UnifiedRouter {
	let router = UnifiedRouter::new();
	#[cfg(server)]
	let router = router.server(|server| server.mount("/", server_router::server_url_patterns()));
	#[cfg(client)]
	let router = router.client(|_| client_router::client_url_patterns());
	router.with_namespace("distribution")
}
