//! Client-side (WASM) modules for auth.
//!
//! - `lib`        — `#[wasm_bindgen(start)]` entry point (delegates to `ClientLauncher`)
//! - `components` — reusable UI components grouped per app

pub mod lib;

pub mod components;
