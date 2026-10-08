//! Live control metadata is deliberately excluded from memory-only archives.
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "memory_unit_origins")]
#[derive(Serialize, Deserialize)]
pub struct MemoryUnitOrigin {
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field]
	pub revision: i64,
	#[field]
	pub authority: Json<super::super::serializers::service::SavedAuthority>,
	#[field(null = true)]
	pub origin_run: Option<Uuid>,
}

#[model(app_label = "knowledge", table_name = "memory_unit_run_origins")]
#[derive(Serialize, Deserialize)]
pub struct MemoryUnitRunOrigin {
	#[field(primary_key = true)]
	pub unit_id: Uuid,
	#[field(primary_key = true)]
	pub run_id: Uuid,
}
