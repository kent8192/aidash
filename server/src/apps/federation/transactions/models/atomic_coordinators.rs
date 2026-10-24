//! Persistent atomic_coordinators records.

use crate::apps::federation::transactions::services::states::AtomicCoordinatorDecision;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "federation", table_name = "atomic_coordinators")]
#[derive(Serialize, Deserialize)]
pub struct AtomicCoordinator {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub manifest: Json<Value>,
	#[field(field_type = "text", max_length = 64, null = true)]
	pub decision: Option<AtomicCoordinatorDecision>,
	#[field(default = false)]
	pub visible: bool,
	#[field(default = false)]
	pub complete: bool,
	#[field(field_type = "text", null = true)]
	pub last_error: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}
