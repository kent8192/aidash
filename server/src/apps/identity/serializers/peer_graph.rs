use serde::{Deserialize, Serialize};
// Serializable peer graph contracts.

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct GraphExpandInput {
	/// The Node address to expand.
	pub node_id: String,
	/// The sending Node of the caller's Tenant; its default Node when absent.
	#[serde(default)]
	pub sending_node: Option<String>,
	#[serde(flatten)]
	pub options: GraphOptions,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct GraphRequest {
	/// The Subject of the sending Tenant that the Federation token names.
	pub(crate) subject: String,
	#[serde(flatten)]
	pub(crate) options: GraphOptions,
}

pub use aidash_domain::federation::graph::{
	GraphActivity, GraphCursor, GraphEdge, GraphNode, GraphOptions, GraphPage, GraphPeer,
};
