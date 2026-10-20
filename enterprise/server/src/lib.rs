//! The Enterprise build's server. It composes the Community server library
//! and runs only the Cloud and Dedicated Offerings.
use aidash_domain::offering::Offering;
use aidash_server::authorization::sign_in::SignInProvider;
use aidash_server::bootstrap::build::{Build, Extension, ProvidedStore};
use aidash_server::database::classification::Table;
use std::sync::Arc;

pub mod billing;
pub mod classification;
pub mod gcip;
pub mod provider_credentials;

pub const BUILD: Build = Build {
	name: "Enterprise",
	offerings: &[Offering::Cloud, Offering::Dedicated],
	manifest_dir: env!("CARGO_MANIFEST_DIR"),
	package: env!("CARGO_PKG_NAME"),
	apps: &["gcip", "billing"],
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
		builder
			.add_source(gcip::ManagedGcipSource::from_env())
			.add_source(
				reinhardt::conf::settings::sources::DefaultSource::new()
					.with_value("billing", billing::settings::sandbox_defaults()),
			)
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

	/// `enterprise/server/routes.json` lists the routes per Offering: sign-in
	/// and the usage report serve both, Billing Accounts only Cloud.
	fn routes(&self, offering: Offering) -> Option<reinhardt::UnifiedRouter> {
		let router = reinhardt::UnifiedRouter::new()
			.mount_unified("/", gcip::routes())
			.mount_unified("/", billing::views::usage_routes());
		Some(match offering {
			Offering::Cloud => router.mount_unified("/", billing::views::account_routes()),
			Offering::Dedicated | Offering::SelfHosted => router,
		})
	}

	fn install(&self, store: &mut aidash_server::store::Store) -> aidash_server::Result<()> {
		billing::install(store, aidash_server::bootstrap::build::offering())
	}

	fn services(
		&self,
		federation: &aidash_server::federation::Federation,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> Vec<aidash_server::bootstrap::build::Service> {
		let store = federation.store.clone();
		let Ok(settings) = billing::load_settings() else {
			return Vec::new();
		};
		vec![Box::pin(billing::meter::maintain(
			store, settings, stopping,
		))]
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
