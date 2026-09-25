//! Typed management responses. Only provenance/configuration payloads are open-ended.
use super::contracts::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConfiguredAgent {
	pub entry: crate::registry::Entry,
	pub catalog_approval_required: bool,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct MaterializedFile {
	pub area_id: Uuid,
	pub revision: i64,
	pub file: FileEntry,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovalPage {
	pub items: Vec<ApprovalCard>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ApprovalDecided {
	pub approval_id: Uuid,
	pub state: String,
	pub revision: i64,
	pub grant_id: Option<Uuid>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct Revoked {
	pub id: Uuid,
	pub state: String,
	pub revision: i64,
	pub effects_may_have_occurred: bool,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReferenceRevoked {
	pub reference_id: Uuid,
	pub status: String,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeletionConfirmation {
	pub confirmation_id: Uuid,
	pub area_id: Uuid,
	pub revision: i64,
	pub expires_at: DateTime<Utc>,
	pub irreversible: bool,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransferStatus {
	pub operation_id: Uuid,
	pub status: String,
	#[serde(flatten)]
	pub transfer: ShareResult,
	pub revision: Option<i64>,
	pub generation: Option<i64>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct TransferPage {
	pub items: Vec<TransferStatus>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct FileRecipient {
	pub node_id: String,
	pub agent_id: String,
	pub agent_version: String,
	pub thread_id: Uuid,
	pub workspace_id: Uuid,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct RecipientPage {
	pub protocol: String,
	pub items: Vec<FileRecipient>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct OperationSummary {
	pub operation_id: Uuid,
	pub kind: String,
	pub status: String,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct OperationPage {
	pub items: Vec<OperationSummary>,
	pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeletedThread {
	pub thread_id: Uuid,
	pub state: String,
	pub file_operations: Vec<super::cleanup::CleanupResult>,
}
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct OutboundStatus {
	pub operation_id: Uuid,
	pub status: String,
	#[serde(flatten)]
	pub outbound: OutboundResult,
	pub url: String,
	pub final_url: Option<String>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn management_schema_has_typed_results_and_distinct_recipient_contracts() {
		let api = serde_json::to_value(crate::api::openapi()).unwrap();
		let schemas = &api["components"]["schemas"];
		assert_eq!(schemas["Recipient"]["additionalProperties"], false);
		assert!(
			schemas["Recipient"]["properties"]
				.get("workspace_id")
				.is_none()
		);
		assert!(
			schemas["FileRecipient"]["required"]
				.as_array()
				.unwrap()
				.contains(&json!("workspace_id"))
		);
		for expected in [
			"core_agent_configure",
			"core_file_materialize",
			"core_approval_list",
			"core_approval_decide",
			"core_approval_revoke",
			"core_grant_revoke",
			"reference_revoke",
			"core_deletion_confirmation",
			"file_transfer_status",
			"file_transfer_history",
			"file_transfer_reconcile",
			"file_recipient_list",
			"core_operation_history",
			"core_thread_delete",
			"core_cleanup_reconcile",
		] {
			let operation = api["paths"]
				.as_object()
				.unwrap()
				.values()
				.flat_map(|path| path.as_object().unwrap().values())
				.find(|op| op["operationId"] == expected)
				.unwrap_or_else(|| panic!("missing {expected}"));
			let response = &operation["responses"]["200"]["content"]["application/json"]["schema"];
			assert!(
				response["$ref"].is_string(),
				"{expected} must have an executable typed response: {response}"
			);
		}
		let _ = std::mem::size_of::<TransferStatus>();
	}
}
