//! Normalize text before declarative request validation.
use serde::{Deserialize, Deserializer};

pub(crate) fn trimmed_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
	Ok(String::deserialize(deserializer)?.trim().to_owned())
}

/// An optional text field: trimmed, and absent when only whitespace.
pub(crate) fn optional_trimmed_text<'de, D: Deserializer<'de>>(
	deserializer: D,
) -> Result<Option<String>, D::Error> {
	Ok(Option::<String>::deserialize(deserializer)?
		.map(|text| text.trim().to_owned())
		.filter(|text| !text.is_empty()))
}
