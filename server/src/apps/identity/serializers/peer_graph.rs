use serde::{Deserialize, Serialize};
// Serializable peer graph contracts.
use crate::apps::identity::services::peer::graph::grant_page_size;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PeerGraphGrant {
	/// The viewer's Deployment id.
	pub source_deployment: String,
	pub source_identity: Uuid,
	/// The key ID of the viewer Deployment's pinned Deployment Key.
	pub source_key_id: String,
	#[serde(skip)]
	pub source_public_key_pem: String,
	pub tenant: String,
	pub enabled: bool,
	pub revision: i64,
	pub updated_at: DateTime<Utc>,
}
crate::native_record!(PeerGraphGrant {
	source_deployment,
	source_identity,
	source_key_id,
	source_public_key_pem,
	tenant,
	enabled,
	revision,
	updated_at
});

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct PeerGraphGrantInput {
	/// The viewer's Deployment id.
	#[validate(length(min = 1, max = 109))]
	pub source_deployment: String,
	pub source_identity: Uuid,
	/// Where the viewer Deployment answers. Required unless disabling.
	#[serde(default)]
	pub endpoint: Option<String>,
	/// The viewer Deployment Key's key ID. Required unless disabling.
	#[serde(default)]
	pub key_id: Option<String>,
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
	/// The Node address to expand; for a Deployment Administrator, the other
	/// Deployment's id.
	pub node_id: String,
	/// The sending Node of the caller's Tenant; its default Node when absent.
	#[serde(default)]
	pub sending_node: Option<String>,
	/// A Deployment Administrator names where the other Deployment answers.
	#[serde(default)]
	pub endpoint: Option<String>,
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
