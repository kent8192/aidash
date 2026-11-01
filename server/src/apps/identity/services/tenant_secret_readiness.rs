//! Readiness facts for the Tenant Secrets one Tenant's definitions name
//! (decision 5 of #310). The facts never carry a value, a Fingerprint or the
//! last four characters, and they read only the caller's Tenant.
use super::access::Access;
use crate::{Result, store::Store};
use aidash_application::tenant_secrets::readiness::{self, Availability, Fact, Target};
use aidash_domain::identity::tenant_administration::{COLLECTION, SECRET_MANAGE};
use serde_json::json;

/// Whether the caller may repair a named Secret here. Tenant Secrets are
/// managed on their collection, the same decision the Tenant Secrets screen
/// needs.
pub(crate) async fn can_manage(access: &mut Access) -> Result<bool> {
	let resource = access.resource("secret", COLLECTION, json!({"name": COLLECTION}));
	access.decide(&resource, SECRET_MANAGE).await
}

fn availability(store: &Store) -> Availability {
	if let Some(service) = &store.tenant_secrets {
		return if service.reader.is_some() || service.calls.is_some() {
			Availability::Available
		} else {
			// A Store that seals hosts with the value needs the broker to attach
			// it; without one the Deployment cannot offer Tenant Secrets.
			Availability::BrokerNotConfigured
		};
	}
	if store
		.provider_credentials
		.as_ref()
		.is_some_and(|service| service.store.seals_secret_hosts())
	{
		Availability::BrokerNotConfigured
	} else {
		Availability::Unavailable
	}
}

/// The facts for `targets`, using only this Tenant's active Tenant Secrets.
pub(crate) async fn facts(
	store: &Store,
	tenant: &str,
	can_manage: bool,
	targets: Vec<Target>,
) -> Result<Vec<Fact>> {
	let availability = availability(store);
	let secrets = match (availability, &store.tenant_secrets) {
		(Availability::Available, Some(service)) => service.active(tenant).await?,
		_ => Vec::new(),
	};
	Ok(readiness::facts(
		availability,
		can_manage,
		targets,
		&secrets,
	))
}
