//! App-owned browser routes.
pub fn client_url_patterns() -> reinhardt::pages::router::ClientRouter {
	use reinhardt::pages::router::ClientRouter;
	ClientRouter::new().route("marketplace", "/", crate::app::view)
}
