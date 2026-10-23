//! Durable generation_remote_intents rows.
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "generation_remote_intents")]
#[derive(Serialize, Deserialize)]
pub struct GenerationRemoteIntents {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "generation_remote_intents_task_id_fkey",
		db_index = false
	)]
	pub task: ForeignKeyField<crate::apps::workspaces::models::Task>,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "generation_remote_intents_credential_id_fkey",
		db_index = false
	)]
	pub credential: ForeignKeyField<crate::apps::identity::models::AuthorizationCredential>,
	#[field(field_type = "text")]
	pub root_subject: String,
	#[field]
	pub subject_chain: Vec<String>,
	#[field]
	pub binding: reinhardt::db::orm::Json<serde_json::Value>,
	#[field]
	pub cancelled: bool,
	#[field]
	pub cancel_delivered: bool,
	#[field]
	pub cancel_retry_at: Option<chrono::DateTime<chrono::Utc>>,
	#[field]
	pub created_at: chrono::DateTime<chrono::Utc>,
}
