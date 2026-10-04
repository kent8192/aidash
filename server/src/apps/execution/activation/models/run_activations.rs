//! Persistent run_activations records.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "run_activations")]
#[derive(Serialize, Deserialize)]
pub struct RunActivations {
	#[field(primary_key = true)]
	pub generation: i64,
	#[field]
	pub id: uuid::Uuid,
	#[field]
	pub run_id: uuid::Uuid,
	#[field]
	pub run_revision: i64,
	#[field(field_type = "text")]
	pub reason: String,
	#[field(field_type = "text")]
	pub state: String,
	#[field(null = true)]
	pub due_at: Option<DateTime<Utc>>,
	#[field]
	pub publication_epoch: i64,
	#[field(null = true)]
	pub publish_token: Option<uuid::Uuid>,
	#[field(null = true)]
	pub publish_until: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub published_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub lease_token: Option<uuid::Uuid>,
	#[field(null = true)]
	pub claimed_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub claim_source: Option<String>,
	#[field(null = true)]
	pub worker_pid: Option<i64>,
	#[field(field_type = "text", null = true)]
	pub disposition: Option<String>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
