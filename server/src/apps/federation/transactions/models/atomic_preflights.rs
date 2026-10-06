//! Persistent atomic_preflights records.
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "federation", table_name = "atomic_preflights")]
#[derive(Serialize, Deserialize)]
pub struct AtomicPreflights {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub binding: Json<Value>,
}
