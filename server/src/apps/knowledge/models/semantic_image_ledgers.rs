//! Durable image quota assignments for one Workspace index.
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "semantic_image_ledgers")]
#[derive(Serialize, Deserialize)]
pub struct SemanticImageLedger {
	#[field(primary_key = true)]
	pub workspace_id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub slots: Json<Value>,
	#[field]
	pub authorities: Json<Value>,
	#[field(default = false)]
	pub initialized: bool,
	#[field(default = false)]
	pub waiting_authority: bool,
	#[field(auto_now_add = true)]
	pub next_authority_check: DateTime<Utc>,
}
