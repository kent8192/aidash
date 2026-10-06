//! Configuration module for server

pub mod apps;
pub mod settings;
#[cfg(feature = "commands-shell")]
pub mod shell;
pub mod urls;

pub mod runtime;
pub(crate) use runtime::same_secret;
pub use runtime::{
	Config, NodeIdentity, OidcConfig, PROTOCOL_VERSION, peer_secret, secret, validate_endpoint,
	validate_node_id, validate_peer_credential, validate_secret_reference,
};

pub mod openapi;

pub mod startup;

pub(crate) const GOOGLE_OIDC_ISSUER: &str = "https://accounts.google.com";

mod legacy_env;
