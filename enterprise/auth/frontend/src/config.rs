//! Native app registry and browser route composition.
#[cfg(not(target_arch = "wasm32"))]
pub mod apps;
pub mod urls;
