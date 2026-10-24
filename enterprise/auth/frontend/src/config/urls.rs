//! Aggregate the frontend's single Pages app.
#[cfg(target_arch = "wasm32")]
pub fn client_routes() -> reinhardt::pages::router::ClientRouter {
	crate::apps::account::urls::client_url_patterns()
}
