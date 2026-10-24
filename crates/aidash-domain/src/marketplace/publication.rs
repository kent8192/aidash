//! Publication metadata bounds remain independent of persistence and HTTP.
use crate::{Error, Result, policy::identifier};
pub fn validate_metadata(
	package_id: &str,
	author: &str,
	dependencies: usize,
	permissions: usize,
) -> Result<()> {
	identifier(package_id)?;
	if author.trim().is_empty() || author.len() > 256 || dependencies > 128 || permissions > 128 {
		return Err(Error::Invalid("invalid publication metadata".into()));
	}
	Ok(())
}

pub fn initial_audience(version: &crate::marketplace::Version) -> crate::marketplace::Audience {
	crate::marketplace::Audience {
		revision: 1,
		tenants: std::collections::BTreeSet::from([version.owner_tenant.clone()]),
	}
}
