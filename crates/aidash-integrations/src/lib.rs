//! External service adapters implementing application ports.
pub mod capability;
pub mod compaction;
pub mod decision;
pub mod federation;
pub mod inference;
pub mod kubernetes;
pub mod openrouter;
mod response;
pub mod semantic;

pub use aidash_application::{Error, Result};

fn http_error(error: reqwest::Error) -> Error {
	// External URLs can contain credentials; never retain them in errors.
	Error::External(error.without_url().to_string())
}

pub mod nats;

pub mod tools;

pub mod skill_import;

pub mod runner;

pub mod outbound;

pub mod sandbox;

pub mod activation;

pub mod oidc;

pub mod gcip;

pub mod provider_credentials;
