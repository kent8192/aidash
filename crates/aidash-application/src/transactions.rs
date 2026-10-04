//! Manifest admission uses the same injected registry-definition structure validator.
use crate::{Result, registry::DefinitionValidation};
use aidash_domain::transactions::Manifest;
pub fn validate(registry: &DefinitionValidation, manifest: &Manifest) -> Result<()> {
	manifest.validate_with(|entry| registry.validate_in(entry, false))
}

pub mod admission;
pub mod authority;
pub mod coordination;
pub mod participation;
