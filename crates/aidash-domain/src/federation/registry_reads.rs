//! Exact remote definition references preserve the Federation verification contract.
use crate::{Error, Result, policy::identifier, registry::EntityRef};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reference {
	pub entry: EntityRef,
	pub digest: String,
}
pub fn validate(references: &[Reference]) -> Result<()> {
	if references.is_empty() || references.len() > 128 {
		return Err(Error::Invalid(
			"verification requires 1..128 references".into(),
		));
	}
	for reference in references {
		identifier(&reference.entry.id)?;
		if semver::Version::parse(&reference.entry.version).is_err()
			|| !reference.digest.starts_with("sha256:")
			|| reference.digest.len() != 71
		{
			return Err(Error::Invalid("invalid remote registry reference".into()));
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;
