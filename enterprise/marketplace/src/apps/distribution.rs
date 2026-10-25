//! Full-stack marketplace app generated with startapp --with-pages.
#[cfg(client)]
pub mod client;
#[cfg(server)]
pub(crate) mod models;
pub mod serializers;
#[cfg(server)]
pub mod server;
pub mod server_fn;
pub mod services;
pub mod urls;
#[cfg(server)]
#[reinhardt::app_config(name = "distribution", label = "marketplace_distribution")]
pub struct DistributionConfig;
