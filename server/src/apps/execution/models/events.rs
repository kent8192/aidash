//! Persistent events records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "execution", table_name = "events")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
	#[field(primary_key = true)]
	pub sequence: i64,
	/// The Tenant the event concerns. A Deployment-level event, such as a peer
	/// registration, concerns none.
	#[field(field_type = "text", null = true)]
	pub tenant: Option<String>,
	#[field]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub node_id: String,

	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "events_workspace_id_fkey",
		db_index = false,
		null = true
	)]
	pub workspace: ForeignKeyField<crate::apps::workspaces::models::Workspace>,
	#[field(field_type = "text")]
	pub kind: String,
	#[field]
	pub data: Json<Value>,
	#[field(null = true)]
	pub published_at: Option<DateTime<Utc>>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub next_attempt_at: DateTime<Utc>,
	#[field(field_type = "text", null = true)]
	pub publish_error: Option<String>,
}
