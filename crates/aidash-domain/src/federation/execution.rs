//! Pinned remote execution definitions, source grants and admission descriptions.
use crate::{
	Task,
	registry::{EntityRef, Entry},
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Definition {
	pub entry: EntityRef,
	pub kind: String,
	pub digest: String,
	pub metadata: Entry,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
	pub binding_snapshot: crate::registry::bindings::BindingSnapshot,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub generation: Option<serde_json::Value>,
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub lineage: Vec<crate::generation::remote::Ancestor>,
	pub node_id: String,
	pub authority_digest: String,
	pub agent: Entry,
	pub definitions: Vec<Definition>,
	#[serde(default, skip_serializing_if = "zero")]
	pub semantic_memory: u32,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub compactor: Option<EntityRef>,
}

fn zero(value: &u32) -> bool {
	*value == 0
}

/// Read-only selection preview. Grant preparation rechecks the current closure.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentInspectionInput {
	pub node_id: String,
	pub agent: EntityRef,
}
#[derive(Serialize, JsonSchema)]
pub struct AgentMemoryRequirements {
	pub native_required: bool,
	pub memory_available: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareInput {
	/// Caller-supplied idempotency key. Reuse requires identical authority and definitions.
	pub id: Uuid,
	pub node_id: String,
	pub agent: EntityRef,
	pub ttl_seconds: i64,
	#[serde(default)]
	pub semantic: crate::semantic::remote::Request,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct Prepared {
	pub id: Uuid,
	pub task_id: Uuid,
	pub node_id: String,
	pub agent: EntityRef,
	pub expires_at: DateTime<Utc>,
	pub revoked: bool,
	pub semantic: crate::semantic::remote::Binding,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
	pub grant_id: Uuid,
	pub source_node: String,
	pub target_node: String,
	pub source_tenant: String,
	pub source_subject: String,
	pub task: Task,
	pub inspection: Inspection,
	pub expires_at: DateTime<Utc>,
	#[serde(
		default,
		skip_serializing_if = "crate::semantic::remote::Binding::disabled"
	)]
	pub semantic: crate::semantic::remote::Binding,
}

impl Inspection {
	pub fn satisfies(&self, pinned: &Self) -> bool {
		self.binding_snapshot == pinned.binding_snapshot
			&& self.generation == pinned.generation
			&& self.lineage == pinned.lineage
			&& self.node_id == pinned.node_id
			&& self.authority_digest == pinned.authority_digest
			&& self.agent == pinned.agent
			&& self.definitions == pinned.definitions
			&& self.compactor == pinned.compactor
			&& (pinned.semantic_memory == 0 || self.semantic_memory == pinned.semantic_memory)
	}
}

#[cfg(test)]
mod tests;

pub mod admission;

pub mod home;
