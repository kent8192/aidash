//! A retained read cannot enlarge a worker's inherited definition approval.
use crate::{Error, Result, ports::catalog::retained::RetainedCatalogScope};
use aidash_domain::registry::{EntityRef, Entry};
pub async fn entry(scope: &mut dyn RetainedCatalogScope, reference: &EntityRef) -> Result<Entry> {
	if scope.inherited() && !scope.approved(reference) {
		return Err(Error::Forbidden);
	}
	let lock = !scope.inherited();
	if scope.enabled(reference, lock).await? != Some(true) {
		return Err(Error::Forbidden);
	}
	let entry = scope
		.definition(reference)
		.await
		.map_err(|error| match error {
			Error::NotFound(_) => Error::Forbidden,
			other => other,
		})?;
	if !scope
		.decide(&scope.resource(&entry), "registry.read")
		.await?
	{
		return Err(Error::Forbidden);
	}
	scope.remember(reference);
	Ok(entry)
}
#[cfg(test)]
mod tests;
