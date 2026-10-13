//! Peer protocol versions and what each one adds on the wire.
//!
//! A node advertises [`CURRENT`] and accepts every version listed in
//! [`compatible`]. Each outbound request carries the version the receiving
//! peer was registered with, so a node never sends a field that peer's software
//! rejects (the wire types use `deny_unknown_fields`).

/// The version this node advertises on `/.well-known/aidash`.
pub const CURRENT: &str = "0.3";

/// Versions an older peer may still speak to a node at `local`.
const ACCEPTED_FROM: &[(&str, &[&str])] = &[(CURRENT, &["0.2"])];

/// First version whose `grants/describe` reply carries the Home's semantic
/// corpus digest (#191).
const CORPUS_DIGEST_SINCE: (u32, u32) = (0, 3);

/// Whether a node speaking `local` serves a peer registered at `peer`.
pub fn compatible(local: &str, peer: &str) -> bool {
	local == peer
		|| ACCEPTED_FROM
			.iter()
			.any(|(version, accepted)| *version == local && accepted.contains(&peer))
}

/// Whether a Home answers a peer speaking `peer` with the corpus digest.
pub fn carries_corpus_digest(peer: &str) -> bool {
	parse(peer).is_some_and(|version| version >= CORPUS_DIGEST_SINCE)
}

fn parse(version: &str) -> Option<(u32, u32)> {
	let (major, minor) = version.split_once('.')?;
	Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests;
