//! API response contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, sqlx::FromRow, JsonSchema)]
pub struct Installation {
	pub id: String,
	pub version: String,
	pub digest: String,
	pub config: Value,
	pub installed_at: chrono::DateTime<chrono::Utc>,
}
