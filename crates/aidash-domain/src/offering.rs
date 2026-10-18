//! How Aidash is provided. One Deployment runs exactly one Offering.
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Offering {
	/// The user runs the Deployment from the Community build.
	#[default]
	SelfHosted,
	/// Aidash runs one shared multi-Tenant Deployment.
	Cloud,
	/// Aidash runs a separate Deployment for one customer.
	Dedicated,
}

impl Offering {
	/// The value the `offering` setting spells.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::SelfHosted => "self-hosted",
			Self::Cloud => "cloud",
			Self::Dedicated => "dedicated",
		}
	}
}

impl fmt::Display for Offering {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(self.as_str())
	}
}

#[cfg(test)]
mod tests {
	use super::Offering;
	use rstest::rstest;

	#[rstest]
	#[case(Offering::SelfHosted, "\"self-hosted\"")]
	#[case(Offering::Cloud, "\"cloud\"")]
	#[case(Offering::Dedicated, "\"dedicated\"")]
	fn the_setting_spells_each_offering_as_displayed(
		#[case] offering: Offering,
		#[case] json: &str,
	) {
		assert_eq!(serde_json::to_string(&offering).unwrap(), json);
		assert_eq!(serde_json::from_str::<Offering>(json).unwrap(), offering);
		assert_eq!(format!("\"{offering}\""), json);
	}

	#[test]
	fn an_absent_offering_is_self_hosted() {
		assert_eq!(Offering::default(), Offering::SelfHosted);
	}
}
