//! Federation identities, offers, and durable delegation state.
use crate::{
	Task,
	registry::{EntityRef, Entry},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Peer {
	pub node_id: String,
	pub endpoint: String,
	pub credential_env: String,
	pub protocol_version: String,
	pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Delegation {
	pub task_id: Uuid,
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub delivered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveredAgent {
	pub node_id: String,
	pub entity: Entry,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Discovery {
	pub agents: Vec<DiscoveredAgent>,
	pub errors: Vec<PeerError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Offer {
	pub task: Task,
	pub agent: EntityRef,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub binding_snapshot: Option<crate::registry::bindings::ForeignAgentSnapshot>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PeerError {
	pub node_id: String,
	pub error: String,
}

pub mod dependencies;

pub mod execution;

pub mod graph;

pub mod registry_reads;
