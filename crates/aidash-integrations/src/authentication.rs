//! Common authentication transport. Local product authorization remains local.
mod client;
pub use client::{Client, Settings};
mod oidc;
pub use oidc::{SourceLogin, exchange_source};

/// Versioned wire contract shared by the authentication server and products.
#[allow(clippy::all)]
pub mod proto {
	include!(concat!(env!("OUT_DIR"), "/aidash.authentication.v1.rs"));
}

#[cfg(test)]
mod tests;
