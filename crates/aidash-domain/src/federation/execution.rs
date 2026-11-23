//! Pinned remote execution definitions, source grants and admission descriptions.
use crate::{
	Task,
	registry::{EntityRef, Entry},
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Receiver-approved, content-free reasons that may cross a Federation boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
	CatalogApproval,
	Policy,
	PolicyAsk,
	NodeUseDenied,
	NodeUseAsk,
	NodeUnreachable,
	InvalidExecutor,
	InstallationInactive,
	ExecutorMismatch,
	DefinitionChanged,
	ReceiverRefused(ReceiverRefusal),
}

/// Only public builtin contract names may appear in a refusal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PublicTool(String);

impl PublicTool {
	pub fn new(operation: &str) -> Option<Self> {
		crate::tool::builtin_reach(operation).map(|_| Self(operation.to_owned()))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl TryFrom<String> for PublicTool {
	type Error = &'static str;

	fn try_from(operation: String) -> Result<Self, Self::Error> {
		crate::tool::builtin_reach(&operation)
			.map(|_| Self(operation))
			.ok_or("unknown public tool")
	}
}

impl From<PublicTool> for String {
	fn from(tool: PublicTool) -> Self {
		tool.0
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolListSide {
	Home,
	Receiver,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiverRefusal {
	ToolNeverRemote {
		#[serde(skip_serializing_if = "Option::is_none")]
		tool: Option<PublicTool>,
	},
	RequiredToolNotListed {
		tool: PublicTool,
		side: ToolListSide,
	},
	BindingInvalid,
	Unclassified,
}

impl ReceiverRefusal {
	pub fn code(&self) -> &'static str {
		match self {
			Self::ToolNeverRemote { .. } => "tool_never_remote",
			Self::RequiredToolNotListed { .. } => "required_tool_not_listed",
			Self::BindingInvalid => "binding_invalid",
			Self::Unclassified => "unclassified",
		}
	}
}

impl Refusal {
	/// Legacy refusals use a bare code. Structured refusals carry only their public fields.
	pub fn header(&self) -> String {
		match self {
			Self::ReceiverRefused(reason) => serde_json::to_string(reason),
			_ => serde_json::to_value(self)
				.map(|value| value.as_str().unwrap_or_default().to_owned()),
		}
		.expect("execution refusal serializes")
	}

	pub fn from_header(value: &str) -> Option<Self> {
		if value.starts_with('{') {
			serde_json::from_str(value).ok().map(Self::ReceiverRefused)
		} else {
			serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
		}
	}
}

impl std::fmt::Display for Refusal {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::CatalogApproval => {
				"the target Node has no Catalog Approval for the agent or its dependencies"
			}
			Self::Policy => {
				"the target Node's policy denies federation.execute or its execution dependencies"
			}
			Self::PolicyAsk => "the target Node requires authority approval for execution",
			Self::NodeUseDenied => "the target Node's policy denies node.use",
			Self::NodeUseAsk => "the target Node requires authority approval for node.use",
			Self::NodeUnreachable => "the target Node is unreachable",
			Self::InvalidExecutor => "the target Agent subject has an incompatible type",
			Self::InstallationInactive => "the target Node has no active installation of the agent",
			Self::ExecutorMismatch => {
				"the target Node's agent does not match the one the task asked for"
			}
			Self::DefinitionChanged => {
				"a dependency of the agent changed on the target Node since it was inspected"
			}
			Self::ReceiverRefused(ReceiverRefusal::ToolNeverRemote { tool }) => {
				let tool = tool.as_ref().map_or("a tool", PublicTool::as_str);
				return write!(
					f,
					"the receiving Node cannot run {tool}, which this Agent uses, remotely"
				);
			}
			Self::ReceiverRefused(ReceiverRefusal::RequiredToolNotListed { tool, side }) => {
				let list = match side {
					ToolListSide::Home => "the Home's Federation Tool Request",
					ToolListSide::Receiver => "the receiving Node's Federation Tool Allowance",
				};
				return write!(
					f,
					"{}, which this Agent needs, is not in {list}",
					tool.as_str()
				);
			}
			Self::ReceiverRefused(ReceiverRefusal::BindingInvalid) => {
				"the receiving Node did not accept this Agent's configuration"
			}
			Self::ReceiverRefused(ReceiverRefusal::Unclassified) => {
				"the receiving Node refused without a reason"
			}
		})
	}
}

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
	/// What the Home asks the receiving Node to let this grant's Runs use (ADR 0139),
	/// as one RFC 9396 entry. Empty when the lists do not apply, as for a receiver of
	/// the Home's own Tenant.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub authorization_details: Vec<super::tool_lists::ToolAuthorizationDetail>,
}

/// The Home's reply to a describe request: the Description a receiver pins at
/// admission, plus the current digest of the Thread's semantic corpus (#191).
///
/// The digest rides beside the Description, not inside it. Admission and every
/// refresh compare the whole Description with the pinned copy, and the corpus
/// changes whenever a Home entry does. It is flattened into the same object.
#[derive(Clone, Serialize)]
pub struct Described {
	#[serde(flatten)]
	pub description: Description,
	/// Absent when the grant carries no Home semantic access.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub corpus_digest: Option<String>,
}
impl Described {
	/// A reply without a digest is valid: the grant carries no Home semantic access.
	pub fn from_value(mut value: serde_json::Value) -> serde_json::Result<Self> {
		let corpus_digest = value
			.as_object_mut()
			.and_then(|fields| fields.remove("corpus_digest"))
			.map(serde_json::from_value)
			.transpose()?;
		Ok(Self {
			description: serde_json::from_value(value)?,
			corpus_digest,
		})
	}
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
			&& (pinned.semantic_memory == 0 || self.semantic_memory == pinned.semantic_memory)
	}
}

#[cfg(test)]
mod tests;

pub mod admission;

pub mod home;

pub mod settlement;
