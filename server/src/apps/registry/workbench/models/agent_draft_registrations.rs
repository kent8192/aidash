//! Persistent agent_draft_registrations records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "registry", table_name = "agent_draft_registrations")]
#[derive(Serialize, Deserialize)]
pub struct AgentDraftRegistration {
	#[field(primary_key = true, field_type = "uuid")]
	pub draft_id: uuid::Uuid,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub version: String,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(field_type = "text")]
	pub release_notes: String,
	#[field(field_type = "text", null = true)]
	pub source_id: Option<String>,
	#[field(field_type = "text", null = true)]
	pub source_version: Option<String>,
	#[field(default = false)]
	pub behavioral_tested: bool,
	#[field(auto_now_add = true)]
	pub registered_at: DateTime<Utc>,
}

impl AgentDraftRegistration {}
