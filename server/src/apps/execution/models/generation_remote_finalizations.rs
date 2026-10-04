//! Durable generation_remote_finalizations rows.
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "execution",
	table_name = "generation_remote_finalizations"
)]
#[derive(Serialize, Deserialize)]
pub struct GenerationRemoteFinalizations {
	#[field(primary_key = true)]
	pub attempt_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub digest: String,
	#[field]
	pub result: Option<reinhardt::db::orm::Json<serde_json::Value>>,
}
