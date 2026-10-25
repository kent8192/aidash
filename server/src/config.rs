//! Configuration module for server

pub mod apps;
pub mod settings;
#[cfg(feature = "commands-shell")]
pub mod shell;
pub mod urls;

pub mod runtime;
pub(crate) use runtime::same_secret;
pub use runtime::{
	Config, DeploymentIdentity, OidcConfig, PROTOCOL_VERSION, SessionConfig, secret,
	validate_endpoint, validate_secret_reference,
};

pub mod openapi;

pub mod startup;

pub(crate) const GOOGLE_OIDC_ISSUER: &str = "https://accounts.google.com";

mod legacy_env;

pub mod marketplace;
