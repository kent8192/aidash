//! Standalone Marketplace application, rendered by Reinhardt Pages in WASM.

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
	console_error_panic_hook::set_once();
	reinhardt::pages::app::ClientLauncher::new("#root")
		.router_client(crate::config::urls::client_routes)
		.launch()
}
