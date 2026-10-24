//! Binary64 bits are written as hexadecimal text, independent of JSON number parsers.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(with = "String")]
pub struct Probability(u64);
impl Probability {
	pub fn new(value: f64) -> Result<Self> {
		if !value.is_finite() || !(0.0..=1.0).contains(&value) {
			return Err(Error::Invalid(
				"Noul probability must be finite and in [0,1]".into(),
			));
		}
		Ok(Self(value.to_bits()))
	}
	pub const fn half() -> Self {
		Self(0x3fe0000000000000)
	}
	pub fn value(self) -> f64 {
		f64::from_bits(self.0)
	}
}
impl Serialize for Probability {
	fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
		s.serialize_str(&format!("{:016x}", self.0))
	}
}
impl<'de> Deserialize<'de> for Probability {
	fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
		let encoded = String::deserialize(d)?;
		if encoded.len() != 16
			|| !encoded
				.bytes()
				.all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
		{
			return Err(serde::de::Error::custom(
				"expected 16 lowercase binary64 hex digits",
			));
		}
		let bits = u64::from_str_radix(&encoded, 16).map_err(serde::de::Error::custom)?;
		Self::new(f64::from_bits(bits)).map_err(serde::de::Error::custom)
	}
}
