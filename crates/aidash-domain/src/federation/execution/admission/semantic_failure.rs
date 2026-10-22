//! Keep the established reason readable while carrying a more specific cause.
use crate::semantic::Failure;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
pub(super) struct Wire {
	#[serde(default)]
	semantic_reason: Option<Failure>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	semantic_cause: Option<Value>,
}

pub(super) fn serialize<S: Serializer>(
	reason: &Option<Failure>,
	serializer: S,
) -> Result<S::Ok, S::Error> {
	Wire {
		semantic_reason: reason.map(Failure::compatible),
		semantic_cause: reason
			.filter(|cause| cause.compatible() != *cause)
			.map(|cause| serde_json::json!(cause)),
	}
	.serialize(serializer)
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
	deserializer: D,
) -> Result<Option<Failure>, D::Error> {
	let wire = Wire::deserialize(deserializer)?;
	Ok(wire
		.semantic_cause
		.and_then(|cause| serde_json::from_value(cause).ok())
		.or(wire.semantic_reason))
}
