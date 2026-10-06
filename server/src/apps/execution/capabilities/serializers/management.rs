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
