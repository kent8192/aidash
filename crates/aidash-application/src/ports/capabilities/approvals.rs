//! Approvals share the caller's live policy, subject chain, record locks and event transaction.
use crate::Result;
use aidash_domain::{
	RunMetadata,
	approval_rules::{ApprovalKind, ApprovalRule, AutoBlocks, Settings},
	capabilities::{operations::MountedFile, records::Record},
	decision::BlockReason,
	policy::{Evaluation, PolicyBundle, Resource},
	provider::ToolCall,
	registry::EntityRef,
	run_input::RunInput,
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
	/// The operation is a declared Read-only operation: it needs no approval,
	/// and the Tenant's policy may still forbid the call outright.
	pub read_only: bool,
}

/// Why a Tool call approval refuses the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
	/// The Tenant's policy forbids the call outright.
	Forbidden,
	/// An Approval Rule of the person the Run's chain starts at denies it.
	Rule,
	/// The person the Run's chain starts at asked not to be asked.
	DontAsk,
	/// No eligible person can decide, so the call cannot be approved.
	Blocked,
	Denied,
	Expired,
	/// The approval no longer holds: revoked, or its approver or grant lapsed.
	Revoked,
	/// The approval ended with its Run.
	Closed,
	/// The approval Decider blocked the call, or gave no verdict.
	Auto(BlockReason),
}
impl Refusal {
	/// What the model is told when the call is refused.
	pub fn message(self) -> &'static str {
		match self {
			Self::Forbidden => "tool call was refused by the Tenant's policy",
			Self::Rule => "tool call was refused by an Approval Rule",
			Self::DontAsk => "tool call was refused: the person asked not to be asked",
			Self::Blocked => "tool call approval was refused: no person is eligible to approve it",
			Self::Denied => "tool call approval was denied",
			Self::Expired => "tool call approval expired",
			Self::Revoked => "tool call approval was revoked",
			Self::Closed => "tool call approval was closed",
			Self::Auto(reason) => reason.refusal_message(),
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

/// A pending approval that the Run's approval Decider is to judge. The harness
/// adds the call and the definition of its tool, which it holds for every kind
/// of tool, Agent Plugin tools included.
#[derive(Debug, Clone)]
pub struct JudgeRequest {
	pub kind: ApprovalKind,
	/// Where the approved call goes: a tool's target, an origin or a disclosure
	/// destination.
	pub target: String,
	/// The inputs people sent to the Run's Thread, in sequence order. Inputs
	/// from other Agents, peers and tools are left out, so nothing a Run read
	/// reaches the judge as a person's words.
	pub people: Vec<RunInput>,
}

/// How the approval Decider settled an approval, with the decision evidence it
/// recorded first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
	/// The call may run: the approval is given in the person's name.
	Allowed { evidence: Uuid },
	/// The call is refused with a reason the model receives.
	Blocked { reason: BlockReason, evidence: Uuid },
	/// The Decider ran in Shadow mode: only its evidence exists, and the
	/// approval stays pending for a person.
	Shadow { evidence: Uuid },
}

#[async_trait]
pub trait ApprovalScope: Send {
	fn principal(&self) -> &str;
	fn credential(&self) -> Uuid;
	fn subjects(&self) -> &[String];
	/// The person the Run's delegation chain starts at. Delegates are
	/// appended, so the root is the first subject.
	fn chain_root(&self) -> Option<&str> {
		self.subjects().first().map(String::as_str)
	}
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
	/// The Approval Mode and Approval Rules that cover `run`: those of the
	/// person its chain starts at, for the Run's Thread. `None` unless that
	/// person is an enabled User subject of this Tenant, so a Run that arrived
	/// through Federation or Tenant maintenance, a Workbench test or an
	/// evaluation fixture is never covered. Read now, so a lowered ceiling or a
	/// removed rule applies to a running Run.
	async fn approval_settings(&mut self, run: &RunMetadata) -> Result<Option<Settings>>;
	/// Store `rule` for `owner`, who reads and writes their own rules. Returns
	/// the id of the rule the owner holds: `rule.id`, or the id of the equal
	/// rule that already exists.
	async fn create_rule(&mut self, owner: &str, rule: ApprovalRule) -> Result<Uuid>;
	/// Every approval record of `run`, of every kind and state.
	async fn approval_records(&mut self, run: &RunMetadata) -> Result<Vec<Record>>;
	/// Every input the Run received, in sequence order, whoever sent it.
	async fn run_inputs(&mut self, run: &RunMetadata) -> Result<Vec<RunInput>>;
	/// What the approval Decider has blocked of `run`'s calls since a person
	/// last approved one. An implementation that can count in its store may
	/// override this.
	async fn auto_blocks(&mut self, run: &RunMetadata) -> Result<AutoBlocks> {
		Ok(AutoBlocks::of(&self.approval_records(run).await?))
	}
}
