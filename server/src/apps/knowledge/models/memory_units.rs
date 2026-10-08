//! Canonical unit bodies are relational text; JSON columns contain typed provenance only.
use aidash_domain::memory::{Entity, Evidence, Link, MentalModel};
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "memory_banks")]
#[derive(Serialize, Deserialize)]
pub struct MemoryBank {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub home: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	#[field(null = true)]
	pub participant_id: Option<Uuid>,
	#[field]
	pub revision: i64,
}

#[model(app_label = "knowledge", table_name = "memory_participants")]
#[derive(Serialize, Deserialize)]
pub struct MemoryParticipant {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub home: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub workspace_id: Uuid,
	#[field(field_type = "text")]
	pub principal: String,
	#[field(field_type = "text")]
	pub agent_id: String,
	#[field(field_type = "text")]
	pub agent_version: String,
	#[field]
	pub revision: i64,
	#[field]
	pub deleted: bool,
}

#[model(app_label = "knowledge", table_name = "memory_units")]
#[derive(Serialize, Deserialize)]
pub struct MemoryUnit {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub text: String,
	#[field(null = true)]
	pub mental_model: Option<Json<MentalModel>>,
	#[field(field_type = "text")]
	pub kind: String,
	#[field(field_type = "text")]
	pub learning: String,
	#[field(field_type = "text")]
	pub verification: String,
	#[field(null = true)]
	pub occurred_start: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub occurred_end: Option<DateTime<Utc>>,
	#[field]
	pub entities: Json<Vec<Entity>>,
	#[field]
	pub evidence: Json<Vec<Evidence>>,
	#[field]
	pub links: Json<Vec<Link>>,
	#[field]
	pub learned_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub deleted: bool,
	#[field]
	pub stale: bool,
}

#[model(app_label = "knowledge", table_name = "memory_run_bindings")]
#[derive(Serialize, Deserialize)]
pub struct MemoryRunBinding {
	#[field(primary_key = true)]
	pub run_id: Uuid,
	#[field]
	pub participant_id: Uuid,
	#[field(field_type = "text")]
	pub provider_id: String,
	#[field(field_type = "text")]
	pub provider_version: String,
	#[field]
	pub participant_revision: i64,
}
