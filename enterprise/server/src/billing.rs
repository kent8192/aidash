//! Cloud billing core: meters, Billing Accounts, Tiers and the rules for who
//! may exist on Cloud. Community supplies two ports, usage events and an
//! admission gate, and this app fills them. A payment processor plugs into
//! [`payments::PaymentState`]; nothing here names one.
use aidash_domain::offering::Offering;
use aidash_server::config::settings::get_scoped_settings;
use aidash_server::federation::Federation;
use aidash_server::store::Store;
use aidash_server::{Error, Result};
use std::sync::{Arc, OnceLock};

pub mod gate;
pub mod meter;
pub mod models;
pub mod nodes;
pub mod payments;
pub mod rules;
pub mod service;
pub mod settings;
pub mod store;
pub mod views;

/// `[billing]` from the Deployment's settings, whose defaults are the sandbox
/// Tiers.
pub fn load_settings() -> Result<settings::Settings> {
	let invalid =
		|error: reinhardt::conf::settings::builder::BuildError| Error::Invalid(error.to_string());
	let value = get_scoped_settings()
		.map_err(invalid)?
		.optional_path::<serde_json::Value>(&["billing"])
		.map_err(invalid)?
		.unwrap_or_else(settings::sandbox_defaults);
	settings::Settings::from_value(value)
}

static SETTINGS: OnceLock<Arc<settings::Settings>> = OnceLock::new();

/// The Deployment's settings, loaded once.
fn shared_settings() -> Result<Arc<settings::Settings>> {
	if let Some(settings) = SETTINGS.get() {
		return Ok(settings.clone());
	}
	let loaded = Arc::new(load_settings()?);
	Ok(SETTINGS.get_or_init(|| loaded).clone())
}

/// The use cases over this process's database, with the processor port its
/// Subscriptions are read through.
pub fn billing(f: &Federation) -> Result<service::Billing> {
	Ok(service::Billing {
		pool: f.store.control_pool.clone(),
		settings: shared_settings()?,
		payments: Arc::new(payments::Mirror {
			pool: f.store.control_pool.clone(),
		}),
		nodes: Arc::new(nodes::DefaultNode),
	})
}

/// Fills Community's usage and admission ports from the Deployment's settings.
/// Dedicated meters like Cloud but its gate admits everything.
pub fn install(store: &mut Store, offering: Offering) -> Result<()> {
	install_with(store, shared_settings()?, offering);
	Ok(())
}

/// Like [`install`], with settings the caller chose, which this process then
/// uses throughout.
pub fn install_with(store: &mut Store, settings: Arc<settings::Settings>, offering: Offering) {
	let _ = SETTINGS.set(settings.clone());
	let pool = store.control_pool.clone();
	store.usage = Arc::new(meter::Meter { pool: pool.clone() });
	store.admission = Arc::new(gate::Gate {
		pool: pool.clone(),
		settings,
		payments: Arc::new(payments::Mirror { pool }),
		offering,
	});
}
