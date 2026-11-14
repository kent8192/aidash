//! Research provenance retained independently of the Context Journal.
use reinhardt::db::{associations::ForeignKeyField, orm::Json};
use reinhardt::model;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[model(app_label = "knowledge", table_name = "memory_journal_proofs")]
#[derive(Serialize, Deserialize)]
pub struct MemoryJournalProof {
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "memory_journal_proofs_run_fkey",
		db_index = false,
		on_delete = Cascade,
		on_update = Restrict
	)]
	pub run: ForeignKeyField<crate::apps::execution::models::Run>,
	#[field]
	pub from_seq: i64,
	#[field]
	pub through_seq: i64,
	#[field]
	pub research: Json<serde_json::Value>,
}
