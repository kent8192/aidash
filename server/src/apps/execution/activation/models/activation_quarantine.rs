//! Persistent activation_quarantine records.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "execution", table_name = "activation_quarantine")]
#[derive(Serialize, Deserialize)]
pub struct ActivationQuarantine {
	#[field(primary_key = true, field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub reason: String,
	#[field(null = true)]
	pub stream_sequence: Option<i64>,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}
