//! Persistent atomic_subjects records.
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "federation", table_name = "atomic_subjects")]
#[derive(Serialize, Deserialize)]
pub struct AtomicSubjects {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field]
	pub binding: Json<Value>,
}
