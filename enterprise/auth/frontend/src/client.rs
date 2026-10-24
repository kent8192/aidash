//! The standalone Authentication Server's Reinhardt Pages browser application.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// ClientLauncher retains the application's reactive scope and event handlers.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn mount() -> Result<(), JsValue> {
	use reinhardt::pages::ClientLauncher;
	ClientLauncher::new("#root")
		.router_client(crate::config::urls::client_routes)
		.launch()
}
