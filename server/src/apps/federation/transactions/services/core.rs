#[path = "authority.rs"]
pub mod authority;
#[path = "coordinator.rs"]
pub mod coordinator;
#[path = "fault.rs"]
pub mod fault;
#[path = "gate.rs"]
pub mod gate;
#[path = "participant.rs"]
pub mod participant;

/// The seed that seals `manifest`'s parts (ADR 0151): an HMAC of the
/// transaction under this Deployment's transaction seed key. The same manifest
/// always gets the same seed, so a resubmission after a crash matches the
/// preflights already sent, and no participant can derive it.
pub fn seed(
	config: &crate::config::Config,
	manifest: &Manifest,
) -> aidash_application::Result<String> {
	seed_with(&config.transaction_seed_key, manifest)
}

/// [`seed`] under `key`.
pub fn seed_with(key: &[u8; 32], manifest: &Manifest) -> aidash_application::Result<String> {
	use hmac::{Hmac, Mac};
	let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key)
		.map_err(|_| aidash_application::Error::External("transaction seed key".into()))?;
	mac.update(manifest.id.as_bytes());
	mac.update(&serde_json::to_vec(manifest)?);
	Ok(format!("{:x}", mac.finalize().into_bytes()))
}

pub fn validate(manifest: &Manifest) -> crate::Result<()> {
	aidash_application::transactions::validate(manifest).map_err(Into::into)
}

pub use crate::apps::federation::transactions::serializers::contracts::{
	Decision, Isolation, LocalStatus, Manifest, Mutation, Participant, Share, Status, Vote,
};
