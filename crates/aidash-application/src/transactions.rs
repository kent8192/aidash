//! Atomic transaction manifests, their admission and coordination.
use crate::Result;
use aidash_domain::transactions::{Manifest, share::Share};
pub fn validate(manifest: &Manifest) -> Result<()> {
	manifest.validate()
}

/// A participant's share, checked against its own seal.
pub fn validate_share(share: &Share) -> Result<()> {
	share.validate().map_err(Into::into)
}

pub mod admission;
pub mod authority;
pub mod coordination;
pub mod management;
pub mod mutation;
pub mod participation;
