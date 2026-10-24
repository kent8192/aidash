//! Project-wide settings, app registry and protocol composition.
pub mod apps;
pub mod settings;
pub mod urls;
pub use settings::{Client, Config, Grpc};
