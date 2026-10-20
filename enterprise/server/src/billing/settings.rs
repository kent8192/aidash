//! `[billing]`: the Tiers a Deployment sells and how long it keeps usage.
use aidash_server::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The Tier of a Personal Tenant whose only Mapping is its Tenant Creator's.
pub const FREE: &str = "free";

const DEFAULT_RETENTION_YEARS: u32 = 7;

/// The payment processor's Price ids for one Tier, per currency. They stay
/// empty here: the Terraform outputs of the processor's objects fill them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prices {
	pub jpy: Option<String>,
	pub usd: Option<String>,
}

/// A named set of limits a Subscription grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tier {
	/// Tenants a Billing Account covers at once. A Tenant in its deletion
	/// grace period still counts.
	pub tenants: u32,
	/// Nodes one covered Tenant may have.
	pub nodes: u32,
	/// Execution Pod seconds included each month.
	pub pod_seconds: u64,
	/// Bytes of stored files included.
	pub storage_bytes: u64,
	#[serde(default)]
	pub prices: Prices,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
	/// How many years metering records, and the attachments of purged
	/// Tenants, are kept.
	#[serde(default = "retention_years")]
	pub usage_retention_years: u32,
	pub tiers: BTreeMap<String, Tier>,
}

fn retention_years() -> u32 {
	DEFAULT_RETENTION_YEARS
}

/// The sandbox test Tiers every Deployment starts with: Free and one paid
/// example. Real prices and limits are a business decision.
pub fn sandbox_defaults() -> serde_json::Value {
	serde_json::json!({
		"usage_retention_years": DEFAULT_RETENTION_YEARS,
		"tiers": {
			FREE: {
				"tenants": 1,
				"nodes": 1,
				"pod_seconds": 3_600,
				"storage_bytes": 1_073_741_824_u64,
			},
			"team": {
				"tenants": 5,
				"nodes": 10,
				"pod_seconds": 360_000,
				"storage_bytes": 53_687_091_200_u64,
			},
		},
	})
}

impl Settings {
	pub fn from_value(value: serde_json::Value) -> Result<Self> {
		let settings: Self = serde_json::from_value(value)
			.map_err(|error| Error::Invalid(format!("invalid [billing]: {error}")))?;
		settings.validate()?;
		Ok(settings)
	}

	fn validate(&self) -> Result<()> {
		let invalid = |message: &str| Err(Error::Invalid(format!("invalid [billing]: {message}")));
		if self.usage_retention_years == 0 {
			return invalid("usage_retention_years must keep records for at least a year");
		}
		let Some(free) = self.tiers.get(FREE) else {
			return invalid("a Free Tier named \"free\" is required");
		};
		if free.tenants != 1 || free.nodes != 1 {
			return invalid("the Free Tier covers one Tenant and one Node");
		}
		for (name, tier) in &self.tiers {
			if name.is_empty()
				|| !name
					.bytes()
					.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
			{
				return invalid("a Tier name is lowercase letters, digits and hyphens");
			}
			if tier.tenants == 0 || tier.nodes == 0 {
				return invalid("a Tier covers at least one Tenant and one Node");
			}
		}
		Ok(())
	}

	pub fn free(&self) -> &Tier {
		&self.tiers[FREE]
	}

	/// A Tier a Subscription can be for: every Tier but Free.
	pub fn paid(&self, name: &str) -> Option<&Tier> {
		(name != FREE).then(|| self.tiers.get(name)).flatten()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_sandbox_defaults_are_valid_and_free_is_not_a_paid_tier() {
		let settings = Settings::from_value(sandbox_defaults()).unwrap();
		assert_eq!(settings.usage_retention_years, 7);
		assert_eq!(settings.free().nodes, 1);
		assert!(settings.paid(FREE).is_none());
		assert_eq!(settings.paid("team").unwrap().tenants, 5);
		assert_eq!(settings.paid("team").unwrap().prices, Prices::default());
		assert!(settings.paid("missing").is_none());
	}

	#[test]
	fn invalid_settings_are_refused_with_their_reason() {
		let mut value = sandbox_defaults();
		value["tiers"].as_object_mut().unwrap().remove(FREE);
		assert!(
			Settings::from_value(value)
				.unwrap_err()
				.to_string()
				.contains("Free Tier")
		);
		let mut value = sandbox_defaults();
		value["tiers"][FREE]["nodes"] = 2.into();
		assert!(Settings::from_value(value).is_err());
		let mut value = sandbox_defaults();
		value["usage_retention_years"] = 0.into();
		assert!(Settings::from_value(value).is_err());
		let mut value = sandbox_defaults();
		value["tiers"]["Team!"] = value["tiers"]["team"].clone();
		assert!(Settings::from_value(value).is_err());
		let mut value = sandbox_defaults();
		value["tiers"]["team"]["unknown"] = 1.into();
		assert!(Settings::from_value(value).is_err());
	}
}
