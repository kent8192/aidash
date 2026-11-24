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

/// A coordinator's per-transaction secret that seals every part (ADR 0146):
/// 64 hex digits from two random (version 4) UUIDs, 244 random bits.
pub fn new_seed() -> String {
	format!(
		"{}{}",
		uuid::Uuid::new_v4().simple(),
		uuid::Uuid::new_v4().simple()
	)
}

pub mod admission;
pub mod authority;
pub mod coordination;
pub mod management;
pub mod mutation;
pub mod participation;
