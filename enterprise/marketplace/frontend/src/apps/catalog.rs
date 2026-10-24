//! Catalog Pages app; sibling backend owns server protocols.
#[cfg(target_arch = "wasm32")]
pub mod client;
pub mod urls;
#[cfg(not(target_arch = "wasm32"))]
#[reinhardt::app_config(name = "catalog", label = "catalog")]
pub struct CatalogConfig;
