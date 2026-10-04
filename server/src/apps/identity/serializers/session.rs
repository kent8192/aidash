//! API response contracts.
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessProfile {
	Operator,
	Subject { tenant: String, subject: String },
}

#[derive(Serialize, JsonSchema)]
pub struct SessionResponse {
	pub access: AccessProfile,
	pub node_id: String,
}
