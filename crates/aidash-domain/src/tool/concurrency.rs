//! Concurrency safety and Resource Claims for Tool Batches. These are provider
//! declarations: independence is never inferred from a read-only effect, replay
//! safety or model-supplied arguments alone.
use super::{Continuation, ToolBehavior, ToolEffect};
use crate::capabilities::{
	files::{FileRead, FileSearch, Representation},
	skills::{SkillList, SkillLoad},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Whether a tool's calls may share a Tool Batch. Registry configuration can
/// only lower a provider's declaration, never raise it.
#[derive(
	Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Concurrency {
	/// Every call runs alone, in the model's order.
	#[default]
	Sequential,
	/// Read-only calls whose provider derives Resource Claims and an output bound.
	SharedRead,
}
impl Concurrency {
	pub fn is_sequential(&self) -> bool {
		*self == Self::Sequential
	}
	/// Apply a Registry restriction. An absent restriction keeps the provider fact.
	pub fn narrowed(self, restriction: Option<Self>) -> Self {
		restriction.map_or(self, |restriction| self.min(restriction))
	}
}

/// Resources are scoped to the calling Run; calls of different Runs never share a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Resource {
	/// The Run's current Working Area, including its manifest revision.
	WorkingArea,
	/// The Run's pinned Skill record, including its per-Skill loaded markers.
	PinnedSkills,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ResourceAccess {
	Shared,
	Exclusive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceClaim {
	pub resource: Resource,
	pub access: ResourceAccess,
}
impl ResourceClaim {
	pub fn shared(resource: Resource) -> Self {
		Self {
			resource,
			access: ResourceAccess::Shared,
		}
	}
	pub fn exclusive(resource: Resource) -> Self {
		Self {
			resource,
			access: ResourceAccess::Exclusive,
		}
	}
	pub fn conflicts(&self, other: &Self) -> bool {
		self.resource == other.resource
			&& (self.access == ResourceAccess::Exclusive
				|| other.access == ResourceAccess::Exclusive)
	}
}

/// What trusted provider code derived for one call that may join a Tool Batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConcurrentCall {
	pub claims: Vec<ResourceClaim>,
	/// Upper bound on result content bytes, excluding the fixed result envelope.
	pub output_bytes: usize,
}
impl ConcurrentCall {
	pub fn conflicts(&self, other: &Self) -> bool {
		self.claims
			.iter()
			.any(|claim| other.claims.iter().any(|held| claim.conflicts(held)))
	}
}

/// A fixed allowance for result bytes outside bounded content: identifiers,
/// status and revisions. File metadata can exceed it. The planner uses it only
/// to choose concurrency; adopted results are charged at their actual size.
pub const RESULT_ENVELOPE_BYTES: usize = 2048;
/// Serialized Skill metadata: bounded name, description, license, origin and identity.
pub const SKILL_METADATA_BYTES: usize = 8704;
/// One `skill_load` inventory entry: a bounded relative path, digest and size.
pub const SKILL_INVENTORY_ENTRY_BYTES: usize = 512;
/// The Skill package limits enforced at import and pinning.
pub const SKILL_INSTRUCTION_BYTES: usize = 65536;
pub const SKILL_PACKAGE_FILES: usize = 64;
pub const SKILL_PAGE_LIMIT: usize = 16;

/// The Node's operator-lowered content ceilings for core read results.
#[derive(Clone, Copy, Debug)]
pub struct ReadCeilings {
	pub read_bytes: usize,
	pub search_bytes: usize,
}

/// A behavior that may share a batch must be an ordinary, unapproved, unfitted read.
pub fn batchable(behavior: &ToolBehavior) -> bool {
	behavior.concurrency == Concurrency::SharedRead
		&& behavior.effect == ToolEffect::ReadOnly
		&& behavior.fitting.is_none()
		&& behavior.continuation == Continuation::Ordinary
		&& !behavior.workbench_approval
}

/// Derive claims and output bounds for core read operations from validated
/// arguments. Anything that cannot be derived runs alone on the sequential path.
pub fn core_call(operation: &str, input: &Value, ceilings: ReadCeilings) -> Option<ConcurrentCall> {
	match operation {
		"file_read" => {
			let read = FileRead::deserialize(input).ok()?;
			let output_bytes = match read.representation {
				Representation::Text => read
					.max_bytes
					.unwrap_or(ceilings.read_bytes)
					.min(ceilings.read_bytes),
				Representation::Metadata => 0,
				// Model media selection changes the next request and stays sequential.
				Representation::ModelInput => return None,
			};
			Some(ConcurrentCall {
				claims: vec![ResourceClaim::shared(Resource::WorkingArea)],
				output_bytes,
			})
		}
		"file_search" => {
			FileSearch::deserialize(input).ok()?;
			Some(ConcurrentCall {
				claims: vec![ResourceClaim::shared(Resource::WorkingArea)],
				output_bytes: ceilings.search_bytes,
			})
		}
		"skill_list" => {
			let list = SkillList::deserialize(input).ok()?;
			let entries = list.limit.unwrap_or(SKILL_PAGE_LIMIT).min(SKILL_PAGE_LIMIT);
			Some(ConcurrentCall {
				claims: vec![ResourceClaim::shared(Resource::PinnedSkills)],
				output_bytes: entries * SKILL_METADATA_BYTES,
			})
		}
		"skill_load" => {
			SkillLoad::deserialize(input).ok()?;
			// Loading rewrites the pinned record's loaded markers.
			Some(ConcurrentCall {
				claims: vec![ResourceClaim::exclusive(Resource::PinnedSkills)],
				output_bytes: SKILL_INSTRUCTION_BYTES
					+ SKILL_METADATA_BYTES
					+ SKILL_PACKAGE_FILES * SKILL_INVENTORY_ENTRY_BYTES,
			})
		}
		_ => None,
	}
}

/// Whether a core read only shares the resources it touches, so its
/// transaction may take shared row locks. Calls that rewrite a resource
/// (`skill_load`) or never batch keep exclusive locks.
pub fn shares_every_resource(operation: &str, input: &Value) -> bool {
	let ceilings = ReadCeilings {
		read_bytes: 0,
		search_bytes: 0,
	};
	core_call(operation, input, ceilings).is_some_and(|call| {
		call.claims
			.iter()
			.all(|claim| claim.access == ResourceAccess::Shared)
	})
}

#[cfg(test)]
mod tests;
