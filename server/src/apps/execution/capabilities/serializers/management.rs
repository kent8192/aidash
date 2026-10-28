use serde::{Deserialize, Serialize};
// Serializable management contracts.
use crate::apps::execution::capabilities::services::core::contracts::*;
use chrono::{DateTime, Utc};

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ConfiguredAgent {
	pub entry: crate::registry::Entry,
	pub catalog_approval_required: bool,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct MaterializedFile {
	pub area_id: Uuid,
	pub revision: i64,
	pub file: FileEntry,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ApprovalCard {
	pub id: Uuid,
	/// `outbound` (an outbound read), `disclosure` (a Web disclosure),
	/// `tool_call` (a call of a Tenant's tool) or `grant` (a Run grant).
	pub kind: String,
	pub run_id: Uuid,
	pub area_id: Option<Uuid>,
	pub state: String,
	pub revision: i64,
	pub requester: String,
	pub approver: Option<String>,
	pub targets: Vec<String>,
	pub action: String,
	pub expires_at: Option<DateTime<Utc>>,
	pub grant_id: Option<Uuid>,
	/// For a Web disclosure approval, the exact query or page address that
	/// would leave the Node.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub disclosure: Option<String>,
	/// Whether the reader is the approver this card is addressed to.
	pub viewer_is_approver: bool,
	pub agent_id: Option<String>,
	pub agent_version: Option<String>,
	/// For a Tool call approval, or a Run grant from one, the Tenant's tool and
	/// its definition version.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub version: Option<String>,
	/// For a Tool call approval of a tool a Run pinned from an Agent Plugin, or a Run grant
	/// from one, the plugin, server and tool, and the plugin copy that declared them. `tool` and
	/// `version` then hold the tool's path and the copy's digest.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub plugin_tool: Option<PluginToolCard>,
	/// The research operation of a Web disclosure approval, or of the
	/// approval a Web disclosure Run grant came from; for a Tool call
	/// approval, the operation of the tool the call invokes.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub operation: Option<String>,
	/// For a Tool call approval, the exact call the approver is asked to allow.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub call: Option<serde_json::Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sampling_target: Option<aidash_domain::agent_plugin::interaction::SamplingTarget>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sampling: Option<aidash_domain::agent_plugin::interaction::SamplingRequest>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider: Option<String>,
	/// The price of this one request, recorded when approval was requested
	/// and charged to the Research account holder: an estimate, not an
	/// invoice. Absent for a provider without a price. The account itself is
	/// kept with the approval for audit and never shown.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub estimated_micro_usd: Option<i64>,
	/// The validated search query in provider-independent form.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub query: Option<serde_json::Value>,
	pub decided_by: Option<String>,
	pub decided_at: Option<DateTime<Utc>>,
	/// What gave the approval in advance, under its owner's name: `{"rule": id}`
	/// for one of their Approval Rules, `{"mode": "bypass"}` for the Bypass
	/// mode, `{"mode": "auto", "decision": id}` for the approval Decider and its
	/// Decision Evidence, and none when a person decided it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub given_by: Option<serde_json::Value>,
	/// What refused the approval in advance, with no one asked: `{"rule": id}`
	/// for one of the owner's Approval Rules, `{"mode": "dont_ask"}` for the
	/// Don't ask mode, and `{"mode": "auto", "reason": ...}` for the approval
	/// Decider, whose reason names a block category or `unavailable`. None when
	/// a person decided it or it is still open.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub refused_by: Option<serde_json::Value>,
	/// Whether the reader may choose "Always allow": they are the person the
	/// Run's delegation chain starts at, the approval is still pending and its
	/// kind can have a rule.
	pub allow_always: bool,
	/// Why a `closed` approval or Run grant was closed, such as
	/// `evidence_revoked` or `run_ended`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub closed_reason: Option<String>,
}

/// The Agent Plugin tool a Tool call approval is asked for.
#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct PluginToolCard {
	pub plugin: String,
	pub server: String,
	/// The server's own name for the tool.
	pub tool: String,
	/// The digest of the plugin copy the Agent version pins.
	pub digest: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ApprovalPage {
	pub items: Vec<ApprovalCard>,
	pub next_cursor: Option<Uuid>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ApprovalDecided {
	pub approval_id: Uuid,
	pub state: String,
	pub revision: i64,
	pub grant_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct Revoked {
	pub id: Uuid,
	pub state: String,
	pub revision: i64,
	pub effects_may_have_occurred: bool,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct ReferenceRevoked {
	pub reference_id: Uuid,
	pub status: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct DeletionConfirmation {
	pub confirmation_id: Uuid,
	pub area_id: Uuid,
	pub revision: i64,
	pub expires_at: DateTime<Utc>,
	pub irreversible: bool,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct TransferStatus {
	pub operation_id: Uuid,
	pub status: String,
	#[serde(flatten)]
	pub transfer: ShareResult,
	pub revision: Option<i64>,
	pub generation: Option<i64>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct TransferPage {
	pub items: Vec<TransferStatus>,
	pub next_cursor: Option<Uuid>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct FileRecipient {
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub thread_id: Uuid,
	pub workspace_id: Uuid,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct RecipientPage {
	pub protocol: String,
	pub items: Vec<FileRecipient>,
	pub next_cursor: Option<Uuid>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct OperationSummary {
	pub operation_id: Uuid,
	pub kind: String,
	pub status: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct OperationPage {
	pub items: Vec<OperationSummary>,
	pub next_cursor: Option<Uuid>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct DeletedThread {
	pub thread_id: Uuid,
	pub state: String,
	pub file_operations: Vec<super::cleanup::CleanupResult>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct OutboundStatus {
	pub operation_id: Uuid,
	pub status: String,
	#[serde(flatten)]
	pub outbound: OutboundResult,
	pub url: String,
	pub final_url: Option<String>,
}

use uuid::Uuid;
