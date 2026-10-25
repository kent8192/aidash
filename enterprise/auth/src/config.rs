//! Project settings and explicit full-stack app registration.
pub mod apps;
#[cfg(server)]
pub mod settings;
pub mod urls;
#[cfg(server)]
pub use settings::{Client, Config, Grpc};
