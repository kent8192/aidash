//! The Enterprise build's server. It composes the Community server library
//! and runs only the Cloud and Dedicated Offerings.
use aidash_domain::offering::Offering;
use aidash_server::authorization::sign_in::SignInProvider;
use aidash_server::bootstrap::build::{Build, Extension, ProvidedStore};
use aidash_server::database::classification::Table;
use std::sync::Arc;

pub mod classification;
pub mod gcip;
pub mod provider_credentials;

pub const BUILD: Build = Build {
	name: "Enterprise",
	offerings: &[Offering::Cloud, Offering::Dedicated],
	manifest_dir: env!("CARGO_MANIFEST_DIR"),
	package: env!("CARGO_PKG_NAME"),
	apps: &["gcip"],
	migrations: &["../enterprise/server/migrations"],
	extension: &Adapters,
};

/// The Google Cloud adapters of Cloud and Dedicated.
#[derive(Debug)]
pub struct Adapters;

impl Extension for Adapters {
	fn settings(
		&self,
		builder: reinhardt::conf::settings::builder::SettingsBuilder,
	) -> reinhardt::conf::settings::builder::SettingsBuilder {
		builder.add_source(gcip::ManagedGcipSource::from_env())
	}

	/// `[dashboard.gcip]`, which only this build reads, from the same sources
	/// as every other setting, including the managed one above.
	fn sign_in(&self) -> aidash_server::Result<Option<Arc<dyn SignInProvider>>> {
		let invalid = |error: reinhardt::conf::settings::builder::BuildError| {
			aidash_server::Error::Invalid(error.to_string())
		};
		let settings = aidash_server::config::settings::get_scoped_settings().map_err(invalid)?;
		settings
			.optional_path::<serde_json::Value>(&["dashboard", "gcip"])
			.map_err(invalid)?
			.map(
				|value| Ok(Arc::new(gcip::Gcip::from_settings(&value)?) as Arc<dyn SignInProvider>),
			)
			.transpose()
	}

	fn owns_issuer(&self, issuer: &str) -> bool {
		aidash_gcp::gcip::is_issuer(issuer)
	}

	/// Every Enterprise route serves both of this build's Offerings so far;
	/// `enterprise/server/routes.json` lists them per Offering.
	fn routes(&self, _offering: Offering) -> Option<reinhardt::UnifiedRouter> {
		Some(gcip::routes())
	}

	fn tables(&self) -> &'static [Table] {
		classification::TABLES
	}

	fn provider_credential_store(
		&self,
		store: &serde_json::Map<String, serde_json::Value>,
		broker: Option<&serde_json::Value>,
	) -> aidash_server::Result<ProvidedStore> {
		provider_credentials::store(store, broker)
	}
}
