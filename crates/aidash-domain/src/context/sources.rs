//! Bounded source reads survive recovery of one inference boundary.
use crate::{Error, Result, registry::rules::digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceObservation {
	pub boundary: String,
	pub binding_digest: String,
	pub content: Value,
	pub digest: String,
}
impl SourceObservation {
	pub fn new(boundary: String, binding_digest: String, content: Value) -> Result<Self> {
		if serde_json::to_vec(&content)?.len() > 1_048_576 {
			return Err(Error::Invalid("source observation exceeds 1 MiB".into()));
		}
		Ok(Self {
			boundary,
			binding_digest,
			digest: digest(&content),
			content,
		})
	}
	pub fn at(&self, boundary: &str, bindings: &str) -> Result<Option<&Value>> {
		if self.digest != digest(&self.content)
			|| serde_json::to_vec(&self.content)?.len() > 1_048_576
		{
			return Err(Error::Invalid(
				"source observation integrity changed".into(),
			));
		}
		Ok((self.boundary == boundary && self.binding_digest == bindings).then_some(&self.content))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	#[test]
	fn durable_roundtrip_recovers_only_the_same_inference_and_binding_graph() {
		let observed = SourceObservation::new(
			"4:9:3".into(),
			"bindings-A".into(),
			json!({"memory":{"fact":"observed"}}),
		)
		.unwrap();
		let recovered: SourceObservation =
			serde_json::from_slice(&serde_json::to_vec(&observed).unwrap()).unwrap();
		assert_eq!(
			recovered.at("4:9:3", "bindings-A").unwrap(),
			Some(&observed.content)
		);
		assert!(recovered.at("5:9:3", "bindings-A").unwrap().is_none());
		assert!(recovered.at("4:10:3", "bindings-A").unwrap().is_none());
		assert!(recovered.at("4:9:3", "bindings-B").unwrap().is_none());
	}
	#[test]
	fn corrupted_or_oversized_observation_cannot_reach_inference() {
		let mut observed = SourceObservation::new(
			"boundary".into(),
			"bindings".into(),
			json!({"memory":"before"}),
		)
		.unwrap();
		observed.content = json!({"memory":"after"});
		assert!(observed.at("boundary", "bindings").is_err());
		assert!(
			SourceObservation::new(
				"boundary".into(),
				"bindings".into(),
				json!("x".repeat(1_048_576))
			)
			.is_err()
		);
	}
}
