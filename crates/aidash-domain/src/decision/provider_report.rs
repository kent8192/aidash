//! Provider-reported facts about a charged physical attempt, never inferred billing.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A finite nonnegative provider USD amount. Binary64 bits survive JSON replay;
/// an absent amount is unknown, while an explicit zero remains a known zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(with = "String")]
pub struct ProviderCost(u64);
impl ProviderCost {
	pub fn new(value: f64) -> Result<Self> {
		if !value.is_finite() || value < 0.0 {
			return Err(Error::Invalid(
				"provider cost must be finite and nonnegative".into(),
			));
		}
		Ok(Self(value.to_bits()))
	}
	pub fn value(self) -> f64 {
		f64::from_bits(self.0)
	}
}
impl Serialize for ProviderCost {
	fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
		serializer.serialize_str(&format!("{:016x}", self.0))
	}
}
impl<'de> Deserialize<'de> for ProviderCost {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
		let encoded = String::deserialize(deserializer)?;
		if encoded.len() != 16
			|| !encoded
				.bytes()
				.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
		{
			return Err(serde::de::Error::custom(
				"expected 16 lowercase binary64 hex digits",
			));
		}
		let bits = u64::from_str_radix(&encoded, 16).map_err(serde::de::Error::custom)?;
		Self::new(f64::from_bits(bits)).map_err(serde::de::Error::custom)
	}
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttemptProviderReport {
	/// No trustworthy report reached the durable attempt. This is never a zero cost.
	#[default]
	Unknown,
	Reported {
		/// Actual validated release/snapshot returned by the provider, not the requested alias.
		served_model: Option<String>,
		/// Actual verified host, canonicalized to its approved slug. None means unknown.
		served_host: Option<String>,
		input_tokens: Option<u64>,
		output_tokens: Option<u64>,
		cost_usd: Option<ProviderCost>,
	},
}
impl AttemptProviderReport {
	pub fn validate_model(&self, requested: &str) -> Result<()> {
		self.validate()?;
		if let Self::Reported {
			served_model: Some(actual),
			..
		} = self && !super::served_by(requested, actual)
		{
			return Err(Error::Invalid(
				"reported model differs from the pinned Route".into(),
			));
		}
		Ok(())
	}
	/// Facts may only name the actual model/host authorized by this immutable Route.
	pub fn validate_route(&self, route: &super::DeciderRoute) -> Result<()> {
		self.validate_model(&route.model)?;
		if let Self::Reported {
			served_host: Some(host),
			..
		} = self && (if route.provider_contract == super::DECISIONS {
			!route.hosts.contains(host)
		} else {
			host != "typesafe"
		}) {
			return Err(Error::Invalid(
				"reported provider differs from the authorized Route".into(),
			));
		}
		Ok(())
	}
	pub fn validate(&self) -> Result<()> {
		if let Self::Reported {
			served_model,
			served_host,
			input_tokens,
			output_tokens,
			cost_usd,
		} = self
		{
			if let Some(model) = served_model {
				crate::provider_credentials::validate_model_id(model)?;
			}
			if let Some(host) = served_host
				&& (host.is_empty()
					|| host.len() > 64
					|| !host
						.bytes()
						.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
			{
				return Err(Error::Invalid("invalid reported provider host".into()));
			}
			if served_model.is_none()
				&& served_host.is_none()
				&& input_tokens.is_none()
				&& output_tokens.is_none()
				&& cost_usd.is_none()
			{
				return Err(Error::Invalid(
					"empty provider report must be unknown".into(),
				));
			}
		}
		Ok(())
	}
}
#[cfg(test)]
mod tests;
