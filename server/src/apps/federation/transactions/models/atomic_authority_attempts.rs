//! Persistent atomic_authority_attempts records.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "atomic_authority_attempts")]
#[derive(Serialize, Deserialize)]
pub struct AtomicAuthorityAttempts {
	#[field(primary_key = true, field_type = "uuid")]
	pub transaction_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub node_id: String,
	#[field(field_type = "text", null = true)]
	pub outcome: Option<String>,
}
