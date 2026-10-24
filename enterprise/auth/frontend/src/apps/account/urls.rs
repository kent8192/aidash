//! App-owned browser routes.
#[cfg(target_arch = "wasm32")]
pub fn client_url_patterns() -> reinhardt::pages::router::ClientRouter {
	use reinhardt::pages::router::ClientRouter;
	ClientRouter::new()
		.route("start", "/", crate::app::page)
		.route("sign-in", "/sign-in", crate::app::page)
		.route("account", "/settings/account", crate::app::page)
}
