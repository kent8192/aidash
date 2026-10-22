//! The people step of a Demo data load (ADR 0079): map each person whose
//! External Identity already exists, and hold a Demo Reservation for the rest.
use super::Person;
use crate::apps::identity::models::DemoReservation;
use crate::database::native::Pool;
use crate::{Error, Result};
use std::collections::BTreeMap;

/// Maps or reserves every person. `tenants` maps each structure key to its
/// assigned Tenant name, and every person's subject already exists in that
/// Tenant's policy with the person's role group.
///
/// Whether an existing External Identity may be mapped now depends on the
/// Deployment's sign-in policy, which `pool` carries.
pub(crate) async fn load(
	pool: &Pool,
	tenants: &BTreeMap<String, String>,
	people: &[Person],
) -> Result<()> {
	let policy = pool.dashboard_policy();
	for person in people {
		let tenant = tenants.get(&person.tenant).ok_or_else(|| {
			Error::Invalid(format!("people names unknown tenant key {}", person.tenant))
		})?;
		DemoReservation::provide(
			pool,
			policy.as_ref(),
			tenant,
			&person.email,
			&person.subject,
		)
		.await?;
	}
	Ok(())
}
