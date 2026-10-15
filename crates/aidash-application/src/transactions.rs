//! Atomic transaction manifests, their admission and coordination.
use crate::Result;
use aidash_domain::transactions::Manifest;
pub fn validate(manifest: &Manifest) -> Result<()> {
	manifest.validate()
}

pub mod admission;
pub mod authority;
pub mod coordination;
pub mod management;
pub mod mutation;
pub mod participation;
