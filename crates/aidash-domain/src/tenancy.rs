//! The identity of a Tenant as the isolation boundary sees it.
use crate::{Result, policy};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The identifier of a Tenant.
///
/// It accepts today's identifier rule, so every Tenant that exists can be
/// named. Assigned Tenant names (#238) will narrow the rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TenantName(String);

impl TenantName {
	pub fn new(value: impl Into<String>) -> Result<Self> {
		let value = value.into();
		policy::identifier(&value)?;
		Ok(Self(value))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn into_inner(self) -> String {
		self.0
	}
}

impl TryFrom<String> for TenantName {
	type Error = crate::Error;
	fn try_from(value: String) -> Result<Self> {
		Self::new(value)
	}
}

impl TryFrom<&str> for TenantName {
	type Error = crate::Error;
	fn try_from(value: &str) -> Result<Self> {
		Self::new(value)
	}
}

impl From<TenantName> for String {
	fn from(value: TenantName) -> Self {
		value.0
	}
}

impl AsRef<str> for TenantName {
	fn as_ref(&self) -> &str {
		&self.0
	}
}

impl fmt::Display for TenantName {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn accepts_today_s_identifiers_including_separators() {
		for name in ["brave-otter-k3m9q2xa", "acme", "a_b", "team:one"] {
			assert_eq!(TenantName::new(name).unwrap().as_str(), name);
		}
	}

	#[test]
	fn rejects_what_the_identifier_rule_rejects() {
		for name in ["", "with space", "wild*", &"x".repeat(257)] {
			assert!(TenantName::new(name).is_err(), "{name:?}");
		}
	}

	#[test]
	fn deserialization_validates() {
		assert!(serde_json::from_str::<TenantName>("\"ok\"").is_ok());
		assert!(serde_json::from_str::<TenantName>("\"no good\"").is_err());
	}
}
