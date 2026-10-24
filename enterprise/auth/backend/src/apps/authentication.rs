//! Human authentication protocols and service-owned persistence.
pub mod grpc;
pub mod http;
pub mod migrations;
pub(crate) mod models;
pub mod service;
pub mod store;
#[cfg(test)]
mod tests;
pub mod urls;
#[reinhardt::app_config(name = "authentication", label = "authentication")]
pub struct AuthenticationConfig;
