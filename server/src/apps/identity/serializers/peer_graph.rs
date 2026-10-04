use serde::{Deserialize, Serialize};
// Serializable peer graph contracts.
use crate::apps::identity::services::peer::graph::grant_page_size;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, sqlx::FromRow, schemars::JsonSchema)]
pub struct GraphOperatorGrant {
	pub source_node: String,
	pub source_operator: Uuid,
	pub tenant: String,
	pub enabled: bool,
	pub revision: i64,
	pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct GraphOperatorGrantInput {
	#[validate(length(min = 1, max = 109))]
	pub source_node: String,
	pub source_operator: Uuid,
	pub enabled: bool,
	#[validate(range(min = 0))]
	pub expected_revision: i64,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrantPage {
	#[serde(default)]
	pub(crate) offset: u64,
	#[serde(default = "grant_page_size")]
	pub(crate) limit: u64,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct GraphExpandInput {
	pub node_id: String,
	#[serde(flatten)]
	pub options: GraphOptions,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct GraphRequest {
	pub(crate) viewer: GraphViewer,
	#[serde(flatten)]
	pub(crate) options: GraphOptions,
}

use uuid::Uuid;

pub use aidash_domain::federation::graph::{
	GraphActivity, GraphCursor, GraphEdge, GraphNode, GraphOptions, GraphPage, GraphPeer,
	GraphViewer,
};
