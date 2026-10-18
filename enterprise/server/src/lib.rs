//! The Enterprise build's server. It composes the Community server library
//! and runs only the Cloud and Dedicated Offerings.
use aidash_domain::offering::Offering;
use aidash_server::bootstrap::build::{Build, Extension, ProvidedStore};

pub mod provider_credentials;

pub const BUILD: Build = Build {
	name: "Enterprise",
	offerings: &[Offering::Cloud, Offering::Dedicated],
	manifest_dir: env!("CARGO_MANIFEST_DIR"),
	package: env!("CARGO_PKG_NAME"),
	apps: &[],
	migrations: &[],
	extension: &Adapters,
};

/// The Google Cloud adapters of Cloud and Dedicated.
#[derive(Debug)]
pub struct Adapters;

impl Extension for Adapters {
	fn provider_credential_store(
		&self,
		store: &serde_json::Map<String, serde_json::Value>,
		broker: Option<&serde_json::Value>,
	) -> aidash_server::Result<ProvidedStore> {
		provider_credentials::store(store, broker)
	}
}
