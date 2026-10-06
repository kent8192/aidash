//! Persistent generation_requests records.

use crate::apps::execution::generation::serializers::contracts::Request;
use crate::apps::execution::generation::services::states::GenerationRequestStatus;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::{DatabaseField, Json, Model, OrmExecutor};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "execution", table_name = "generation_requests")]
#[derive(Serialize, Deserialize)]
pub struct GenerationRequest {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub home_node: String,
	#[field]
	pub foreign_intent: Option<Json<Value>>,
	#[field]
	pub grant_id: Option<uuid::Uuid>,
	#[field]
	pub admission_id: Option<uuid::Uuid>,
	#[field(default = false)]
	pub prepared: bool,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(field_type = "text", db_column = "policy_id")]
	pub policy_key: String,
	#[field]
	pub policy_revision: i64,
	#[field]
	pub task_id: uuid::Uuid,
	#[field]
	pub workspace_id: uuid::Uuid,
	#[field]
	pub credential_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field]
	pub definition: Json<Value>,
	#[field(field_type = "text", max_length = 64)]
	pub status: GenerationRequestStatus,
	#[field(field_type = "text")]
	pub reason: String,
	#[field]
	pub depth: i32,
	#[field]
	pub token_limit: i64,
	#[field(default = false)]
	pub quota_released: bool,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,

	#[field]
	pub retired_catalog_revision: Option<i64>,

	#[field(
		generated_sql = "\nCASE\n    WHEN (home_node = ''::text) THEN task_id\n    ELSE NULL::uuid\nEND",
		generated_stored = true
	)]
	pub local_task_id: Option<uuid::Uuid>,

	#[field(
		generated_sql = "\nCASE\n    WHEN (home_node = ''::text) THEN workspace_id\n    ELSE NULL::uuid\nEND",
		generated_stored = true
	)]
	pub local_workspace_id: Option<uuid::Uuid>,
}

impl GenerationRequest {
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		workspace: Uuid,
	) -> Result<Option<Request>> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.filter(Self::field_workspace_id().eq(workspace))
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.map(Self::contract)
			.transpose()
	}
	pub(crate) fn contract(self) -> Result<Request> {
		Ok(Request {
			id: self.id,
			home_node: self.home_node,
			foreign_intent: self.foreign_intent.map(|value| value.0),
			grant_id: self.grant_id,
			admission_id: self.admission_id,
			prepared: self.prepared,
			task_id: self.task_id,
			workspace_id: self.workspace_id,
			credential_id: self.credential_id,
			tenant: self.tenant,
			policy_id: self.policy_key,
			policy_revision: self.policy_revision,
			root_subject: self.root_subject,
			subject_chain: self.subject_chain,
			agent_id: self.agent_id,
			agent_version: self.agent_version,
			definition: self.definition.0,
			status: self
				.status
				.encode_database()
				.map_err(|error| Error::External(error.to_string()))?,
			reason: self.reason,
			depth: self.depth,
			token_limit: self.token_limit,
			quota_released: self.quota_released,
			expires_at: self.expires_at,
			created_at: self.created_at,
		})
	}

	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		offset: usize,
	) -> Result<Vec<Request>> {
		Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.order_by(&["-created_at", "id"])
			.limit(200)
			.offset(offset)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(Self::contract)
			.collect()
	}
}

impl GenerationRequest {}
