//! Persistent semantic_run_reads records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "knowledge", table_name = "semantic_run_reads")]
#[derive(Serialize, Deserialize)]
pub struct SemanticRunRead {
	#[field(primary_key = true, field_type = "uuid")]
	pub run_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "uuid")]
	pub entry_id: uuid::Uuid,
	#[field(primary_key = true)]
	pub revision: i64,
}

impl SemanticRunRead {}
