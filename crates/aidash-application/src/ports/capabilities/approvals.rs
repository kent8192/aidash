//! Approvals share the caller's live policy, subject chain, record locks and event transaction.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::MountedFile, records::Record},
	policy::{Evaluation, PolicyBundle, Resource},
	provider::ToolCall,
	registry::EntityRef,
	tool::AgentPluginTool,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
pub struct Limits {
	pub admission: bool,
	pub origins: Vec<String>,
	pub approval_seconds: u64,
	pub grant_seconds: u64,
}
pub struct Creation {
	pub id: Uuid,
	pub area: Option<Uuid>,
	pub kind: &'static str,
	pub state: &'static str,
	pub data: Value,
	pub expires: Option<DateTime<Utc>>,
}
/// The tool a Tool call approval is asked for. A Run grant covers one operation of one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallTarget {
	/// A version of a Tenant's HTTP or MCP tool definition in the Registry.
	Registry(EntityRef),
	/// A tool a Run pinned from a remote MCP server of an Agent Plugin copy. It is no Registry
	/// entry: the pin, not a definition, says whether it is Read-only.
	AgentPlugin(AgentPluginTool),
}

impl ToolCallTarget {
	/// What the Tenant's policy, an Approval Rule and the dashboard call the tool: a definition
	/// id, or `agent-plugin:<plugin>/<server>/<tool>`.
	pub fn id(&self) -> String {
		match self {
			Self::Registry(reference) => reference.id.clone(),
			Self::AgentPlugin(tool) => {
				format!("agent-plugin:{}/{}/{}", tool.plugin, tool.server, tool.tool)
			}
		}
	}

	/// What a call is approved for: the definition version, or the digest of the plugin copy
	/// that declared the tool, so another copy of the same plugin needs its own approval.
	pub fn version(&self) -> String {
		match self {
			Self::Registry(reference) => reference.version.clone(),
			Self::AgentPlugin(tool) => tool.digest.clone(),
		}
	}

	/// The `<id>@<version>` an approval prompt names. A plugin copy shows the first 12 hex
	/// digits of its digest.
	pub fn label(&self) -> String {
		match self {
			Self::Registry(_) => format!("{}@{}", self.id(), self.version()),
			Self::AgentPlugin(tool) => {
				let digest = tool.digest.strip_prefix("sha256:").unwrap_or(&tool.digest);
				format!(
					"{}@{}",
					self.id(),
					digest.chars().take(12).collect::<String>()
				)
			}
		}
	}

	/// What an approval record and the Run grant made from it keep to name the target.
	pub fn record(&self) -> Value {
		let mut fields = serde_json::json!({"tool": self.id(), "version": self.version()});
		if let Self::AgentPlugin(tool) = self {
			fields["plugin_tool"] = serde_json::json!(tool);
		}
		fields
	}

	/// The target an approval record or Run grant names, as [`Self::record`] wrote it.
	pub fn from_record(data: &Value) -> Result<Self> {
		if data["plugin_tool"].is_object() {
			return Ok(Self::AgentPlugin(serde_json::from_value(
				data["plugin_tool"].clone(),
			)?));
		}
		Ok(Self::Registry(EntityRef {
			id: data["tool"].as_str().unwrap_or_default().into(),
			version: data["version"].as_str().unwrap_or_default().into(),
		}))
	}
}

/// One exact call of a Tenant's HTTP or MCP tool, or of an Agent Plugin's, that needs a Tool call
/// approval.
#[derive(Debug, Clone)]
pub struct ToolCallRequest {
	/// The call's invocation key. One approval belongs to one key and one call.
	pub key: String,
	pub tool: ToolCallTarget,
	/// The operation of the tool the call invokes; a Run grant covers one.
	pub operation: String,
	pub call: ToolCall,
	/// The Run's Working Area, so the approval appears in its Thread.
	pub area: Option<Uuid>,
}

/// Why a Tool call approval refuses the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
	/// The Tenant's policy forbids the call outright.
	Forbidden,
	/// No eligible person can decide, so the call cannot be approved.
	Blocked,
	Denied,
	Expired,
	/// The approval no longer holds: revoked, or its approver or grant lapsed.
	Revoked,
	/// The approval ended with its Run.
	Closed,
}
impl Refusal {
	/// What the model is told when the call is refused.
	pub fn message(self) -> &'static str {
		match self {
			Self::Forbidden => "tool call was refused by the Tenant's policy",
			Self::Blocked => "tool call approval was refused: no person is eligible to approve it",
			Self::Denied => "tool call approval was denied",
			Self::Expired => "tool call approval expired",
			Self::Revoked => "tool call approval was revoked",
			Self::Closed => "tool call approval was closed",
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallApproval {
	/// The call may run. `approval_id` names the approval it consumed, if any;
	/// a call the Tenant's policy permits directly has none.
	Approved {
		approval_id: Option<Uuid>,
	},
	/// An eligible person must decide first. The Run waits on this approval.
	Pending {
		approval_id: Uuid,
		expires_at: DateTime<Utc>,
	},
	Refused(Refusal),
}

#[async_trait]
pub trait ApprovalScope: Send {
	fn principal(&self) -> &str;
	fn credential(&self) -> Uuid;
	fn subjects(&self) -> &[String];
	fn replace_subjects(&mut self, subjects: Vec<String>) -> Vec<String>;
	fn policy_revision(&self) -> i64;
	fn bundle(&self) -> &PolicyBundle;
	fn evaluation(&self, subject: &str, target: &Resource, action: &str) -> Evaluation;
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource;
	fn limits(&self) -> Result<Limits>;
	async fn run_context(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn interaction_run(&mut self, id: Uuid) -> Result<RunMetadata>;
	async fn context_authority(&mut self) -> Result<()>;
	async fn active_run(&mut self) -> Result<Option<Uuid>>;
	async fn require(&mut self, target: &Resource, action: &str) -> Result<()>;
	async fn load(&mut self, id: Uuid, kind: &str) -> Result<Record>;
	async fn grants(&mut self, run: Uuid) -> Result<Vec<Record>>;
	async fn insert(&mut self, creation: Creation) -> Result<Record>;
	async fn transfer_owner(&mut self, id: Uuid, owner: &str) -> Result<()>;
	async fn update(&mut self, record: &mut Record) -> Result<()>;
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>>;
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()>;
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()>;
	async fn read(&mut self, file: &MountedFile) -> Result<Vec<u8>>;
}
