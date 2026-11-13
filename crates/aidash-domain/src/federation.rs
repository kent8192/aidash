//! Federation identities and durable delegation state.
use crate::registry::Entry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Delegation {
	pub task_id: Uuid,
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscoveredAgent {
	pub node_id: String,
	pub entity: Entry,
}

/// What `agent_discover` hands a model about an Agent: enough to choose one for `task_delegate`,
/// and none of its instructions, bindings or normalization. People's discovery and the
/// responses between Nodes still carry the whole definition (#769).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct AgentCard {
	pub id: String,
	pub version: String,
	pub kind: String,
	pub name: crate::registry::Localized,
	pub description: crate::registry::Localized,
	pub capabilities: Vec<String>,
	pub tags: Vec<String>,
	pub languages: Vec<String>,
	pub skills: Vec<String>,
	/// The id of the model the Agent runs on, when its configuration names one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub model: Option<String>,
}

impl From<&Entry> for AgentCard {
	fn from(entry: &Entry) -> Self {
		Self {
			id: entry.id.clone(),
			version: entry.version.clone(),
			kind: entry.kind.clone(),
			name: entry.name.clone(),
			description: entry.description.clone(),
			capabilities: entry.capabilities.clone(),
			tags: entry.tags.clone(),
			languages: entry.languages.clone(),
			skills: entry.skills.clone(),
			model: entry
				.config
				.pointer("/model/id")
				.and_then(serde_json::Value::as_str)
				.map(str::to_owned),
		}
	}
}

/// A [`DiscoveredAgent`] as a model sees it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct DiscoveredCard {
	pub node_id: String,
	pub entity: AgentCard,
}

/// A [`Discovery`] as a model sees it: the same `{agents:[{node_id, entity}], errors}` shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub struct DiscoveryCards {
	pub agents: Vec<DiscoveredCard>,
	pub errors: Vec<PeerError>,
}

impl From<&Discovery> for DiscoveryCards {
	fn from(discovery: &Discovery) -> Self {
		Self {
			agents: discovery
				.agents
				.iter()
				.map(|agent| DiscoveredCard {
					node_id: agent.node_id.clone(),
					entity: AgentCard::from(&agent.entity),
				})
				.collect(),
			errors: discovery.errors.clone(),
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Discovery {
	pub agents: Vec<DiscoveredAgent>,
	pub errors: Vec<PeerError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PeerError {
	pub node_id: String,
	pub error: String,
}

pub mod dependencies;

pub mod execution;

pub mod graph;

pub mod protocol;

pub mod preview;

pub mod registry_reads;

pub mod route;

#[cfg(test)]
mod tests;
