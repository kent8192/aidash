//! External protocol contracts.
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NodeIdentity {
	pub id: String,
	pub endpoint: String,
	pub capabilities: Vec<String>,
	pub clusters: Vec<String>,
	pub protocol_version: String,
}
