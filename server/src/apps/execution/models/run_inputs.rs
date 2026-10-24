//! Persistent run_inputs records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "run_inputs")]
#[derive(Serialize, Deserialize)]
pub struct RunInput {
	#[field(primary_key = true)]
	pub seq: i64,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "run_inputs_run_id_fkey",
		db_index = false,
		on_delete = Cascade
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field(field_type = "text")]
	pub sender: String,
	#[field(field_type = "text")]
	pub content: String,
	#[field(null = true)]
	pub message_id: Option<uuid::Uuid>,
	#[field(field_type = "text")]
	pub idempotency_key: String,
	#[field(null = true)]
	pub delivery_retry_at: Option<DateTime<Utc>>,
	#[field(default = false)]
	pub reference_only: bool,
}

impl RunInput {}
