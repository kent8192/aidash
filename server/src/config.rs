//! Configuration module for server

pub mod apps;
pub mod settings;
#[cfg(feature = "commands-shell")]
pub mod shell;
pub mod urls;

pub mod prompt_cache;
pub mod runtime;
pub use prompt_cache::{PromptCacheKey, PromptCacheScope, validate_prompt_cache_key};
pub(crate) use runtime::same_secret;
pub use runtime::{
	Config, GcipConfig, NodeIdentity, OidcConfig, PROTOCOL_VERSION, SessionConfig, peer_secret,
	secret, validate_endpoint, validate_node_id, validate_peer_credential,
	validate_secret_reference,
};

pub mod openapi;

pub mod startup;

pub(crate) const GOOGLE_OIDC_ISSUER: &str = "https://accounts.google.com";

mod legacy_env;
