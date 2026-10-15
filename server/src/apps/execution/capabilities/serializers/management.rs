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
	/// The research operation of a Web disclosure approval, or of the
	/// approval a Web disclosure Run grant came from.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub operation: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider: Option<String>,
	/// The Search service operator's account, as recorded when approval was
	/// requested. Absent for a page read.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub operator_account: Option<String>,
	/// The recorded price of this one request, an estimate and not an invoice.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub estimated_micro_usd: Option<i64>,
	/// The validated search query in provider-independent form.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub query: Option<serde_json::Value>,
	pub decided_by: Option<String>,
	pub decided_at: Option<DateTime<Utc>>,
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
