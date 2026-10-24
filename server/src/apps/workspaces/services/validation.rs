//! Normalize text before declarative request validation.
use serde::{Deserialize, Deserializer};

pub(crate) fn trimmed_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
	Ok(String::deserialize(deserializer)?.trim().to_owned())
}
