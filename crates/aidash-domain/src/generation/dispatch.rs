//! Durable provider dispatch bindings and exact settlement messages.
use super::remote::{Finalization, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalizeInput {
	pub usage: Usage,
	pub result: Finalization,
}
