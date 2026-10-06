//! API response contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct Installation {
	pub id: String,
	pub version: String,
	pub digest: String,
	pub config: Value,
	pub installed_at: chrono::DateTime<chrono::Utc>,
}
crate::native_record!(Installation {
	id,
	version,
	digest,
	config,
	installed_at
});
