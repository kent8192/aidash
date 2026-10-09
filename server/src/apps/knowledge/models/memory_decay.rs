//! Usage and recall state are independent of canonical Unit revisions.
use aidash_domain::registry::EntityRef;
use chrono::{DateTime, Utc};
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "memory_unit_retention")]
#[derive(Serialize, Deserialize)]
pub struct MemoryUnitRetention {
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field]
	pub bank_id: Uuid,
	#[field]
	pub deliveries: i64,
	#[field(null = true)]
	pub last_delivered_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub reactivated_at: Option<DateTime<Utc>>,
	#[field]
	pub pinned: bool,
	#[field(null = true)]
	pub dormant_policy: Option<Json<EntityRef>>,
	#[field]
	pub changed_at: DateTime<Utc>,
}

#[model(app_label = "knowledge", table_name = "memory_bank_decay")]
#[derive(Serialize, Deserialize)]
pub struct MemoryBankDecay {
	#[field(primary_key = true)]
	pub bank_id: Uuid,
	#[field]
	pub policy: Json<EntityRef>,
	#[field(null = true)]
	pub activated_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub as_of: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub cursor: Option<Uuid>,
	#[field]
	pub next_job: DateTime<Utc>,
}
