//! Persistent agent_draft_shares records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "registry", table_name = "agent_draft_shares")]
#[derive(Serialize, Deserialize)]
pub struct AgentDraftShare {
	#[field(primary_key = true, field_type = "uuid")]
	pub draft_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub subject: String,
	#[field(default = false)]
	pub can_edit: bool,
	#[field(field_type = "text")]
	pub documents_digest: String,
}

impl AgentDraftShare {}
