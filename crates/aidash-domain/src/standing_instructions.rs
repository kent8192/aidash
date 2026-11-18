//! Tenant Instructions and Node Instructions (#1010): standing directions for
//! every Agent of a Tenant and for every Agent working in one Node. Only Tenant
//! Administrators change a Tenant's text and only that Node's administrators
//! change a Node's; Agents never change either. A Run pins both texts when it
//! is claimed, so editing them never changes a Run already under way.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The largest Tenant or Node Instructions text, in bytes. Both texts count
/// against every request's context window and its Stable Prefix.
pub const MAX_STANDING_INSTRUCTIONS_BYTES: usize = 16_384;

/// The Tenant and Node Instructions one Run works under.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingInstructions {
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub tenant: String,
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub node: String,
}

impl StandingInstructions {
	/// Whether neither text says anything.
	pub fn is_empty(&self) -> bool {
		self.tenant.trim().is_empty() && self.node.trim().is_empty()
	}
}

/// Checks one Tenant or Node Instructions text before it is stored.
pub fn validate_standing_text(text: &str) -> Result<()> {
	if text.len() > MAX_STANDING_INSTRUCTIONS_BYTES {
		return Err(Error::Invalid(format!(
			"instructions must be at most {MAX_STANDING_INSTRUCTIONS_BYTES} bytes"
		)));
	}
	if text.contains('\0') {
		return Err(Error::Invalid(
			"instructions must not contain NUL characters".into(),
		));
	}
	Ok(())
}

#[cfg(test)]
mod tests;
