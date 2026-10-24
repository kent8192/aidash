//! The standalone Authentication Server's Reinhardt Pages browser application.
#[cfg(target_arch = "wasm32")]
mod model;

#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod bridge;
#[cfg(target_arch = "wasm32")]
mod view;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// ClientLauncher retains the application's reactive scope and event handlers.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn mount() -> Result<(), JsValue> {
	use reinhardt::pages::{ClientLauncher, router::ClientRouter};
	ClientLauncher::new("#root")
		.router_client(|| {
			ClientRouter::new()
				.route("start", "/", app::page)
				.route("sign-in", "/sign-in", app::page)
				.route("account", "/settings/account", app::page)
		})
		.launch()
}
