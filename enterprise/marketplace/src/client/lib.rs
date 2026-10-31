// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! WASM entry point for marketplace.
//!
//! Bootstraps the SPA via `ClientLauncher::register_routes_from_inventory`.
//! Each app exposes its own client router through `apps/<app>/urls.rs`;
//! combine them in
//! `src/config/urls.rs` so the `#[routes]` registration owns both server and
//! client route aggregation.

use reinhardt::pages::ClientLauncher;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
	console_error_panic_hook::set_once();
	ClientLauncher::new("#root")
		.register_routes_from_inventory()
		.launch()
}
