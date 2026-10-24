//! The Federation protocol version.
//!
//! A Node speaks [`CURRENT`] and nothing else. The wire types use
//! `deny_unknown_fields`, so a mixed-version mesh could not exchange them, and
//! Aidash is undeployed, so no older version is kept alive.

/// The version this Deployment advertises on `/.well-known/aidash` and every
/// Federation token carries.
pub const CURRENT: &str = "0.4";

/// Whether a Node speaking `local` serves a request that names `peer`.
pub fn compatible(local: &str, peer: &str) -> bool {
	local == peer
}

#[cfg(test)]
mod tests;
