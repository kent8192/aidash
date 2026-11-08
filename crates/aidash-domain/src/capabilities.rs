//! Declared agent capabilities and immutable knowledge attachments.
use crate::registry::SkillFile;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CoreCapabilities {
	pub outbound: bool,
	pub files: bool,
	pub shell: bool,
	pub python: bool,
	pub patch: bool,
	pub skills: bool,
	pub sharing: bool,
}

impl CoreCapabilities {
	pub fn enabled(&self) -> bool {
		self.files
			|| self.shell
			|| self.python
			|| self.patch
			|| self.skills
			|| self.sharing
			|| self.outbound
	}

	pub fn permits(&self, name: &str) -> bool {
		crate::tool::builtin_contract(name)
			.and_then(|contract| contract.authorization.core)
			.is_some_and(|permission| permission.permitted(self))
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillAttachment {
	pub skill_id: Uuid,
	pub origin: String,
	pub digest: String,
	pub instructions: String,
	pub files: Vec<SkillFile>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Attachment")]
pub struct ReferenceAttachment {
	pub reference_id: Uuid,
	pub digest: String,
	/// Where an Agent sees this file, relative to its Reference set.
	pub path: String,
}

pub mod references;
pub mod skills;

pub mod operations;

pub mod outbound;
pub mod records;

pub mod approvals;

pub mod reclamation;

pub mod retention;

pub mod sessions;

pub mod cleanup;

pub mod packages;
pub mod python;

pub mod errors;
pub mod files;

pub mod configuration;
pub mod patch;

pub mod thread_lifecycle;

pub mod sharing;

pub mod transfer;

/// Canonical SHA-256 identity. Parsing rejects alternate encodings and paths.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Digest([u8; 32]);

impl Digest {
	pub fn from_bytes(bytes: [u8; 32]) -> Self {
		Self(bytes)
	}
	pub fn as_bytes(&self) -> &[u8; 32] {
		&self.0
	}
}

impl std::fmt::Display for Digest {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		for byte in self.0 {
			write!(f, "{byte:02x}")?;
		}
		Ok(())
	}
}

impl std::str::FromStr for Digest {
	type Err = &'static str;
	fn from_str(value: &str) -> Result<Self, Self::Err> {
		if value.len() != 64
			|| !value
				.bytes()
				.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
		{
			return Err("invalid SHA-256 digest");
		}
		let mut bytes = [0; 32];
		for (i, byte) in bytes.iter_mut().enumerate() {
			*byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)
				.map_err(|_| "invalid SHA-256 digest")?;
		}
		Ok(Self(bytes))
	}
}

#[cfg(test)]
mod digest_tests {
	use super::Digest;

	#[test]
	fn digest_accepts_only_canonical_sha256_hex() {
		let value = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
		assert_eq!(value.parse::<Digest>().unwrap().to_string(), value);
		assert!("../etc/passwd".parse::<Digest>().is_err());
		assert!(value.to_uppercase().parse::<Digest>().is_err());
		assert!(value[..63].parse::<Digest>().is_err());
	}
}
