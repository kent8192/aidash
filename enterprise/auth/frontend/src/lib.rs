//! Standalone Pages application.
pub mod apps;
#[cfg(target_arch = "wasm32")]
mod client;
pub mod config;
#[cfg(target_arch = "wasm32")]
pub(crate) use apps::account::client::{api, app, bridge, model, view};
