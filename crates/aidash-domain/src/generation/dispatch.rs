//! Durable provider dispatch bindings and exact settlement messages.
use super::remote::{Finalization, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Persistence adapters convert their own projection into this business record.
#[derive(Debug, Clone)]
pub struct Record {
	pub usage: Value,
	pub digest: String,
	pub peer_node: String,
	pub boundary: Value,
	pub state: String,
	pub finalization: Option<Value>,
	pub peer_finalized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
	pub usage: Usage,
	pub boundary: Value,
}

/// The boundary of an inference or compaction admission. On a remote Ordered
/// Run's retrieval-reuse step it also names the semantic operation whose
/// retrieval the step reuses (ADR 0020). Without a reuse it serializes as
/// `{"step":n}`, the only shape older peers send or read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepBoundary {
	pub step: i32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reuses: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalizeInput {
	pub usage: Usage,
	pub result: Finalization,
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn a_step_boundary_without_a_reuse_keeps_the_shape_older_peers_send() {
		let boundary = StepBoundary {
			step: 3,
			reuses: None,
		};
		assert_eq!(serde_json::to_value(boundary).unwrap(), json!({"step":3}));
		assert_eq!(
			serde_json::from_value::<StepBoundary>(json!({"step":3})).unwrap(),
			boundary
		);
	}

	#[test]
	fn a_reuse_step_names_the_reused_operation() {
		let operation = Uuid::from_u128(7);
		let boundary = StepBoundary {
			step: 3,
			reuses: Some(operation),
		};
		let value = serde_json::to_value(boundary).unwrap();
		assert_eq!(value, json!({"step":3,"reuses":operation}));
		assert_eq!(
			serde_json::from_value::<StepBoundary>(value).unwrap(),
			boundary
		);
	}
}
