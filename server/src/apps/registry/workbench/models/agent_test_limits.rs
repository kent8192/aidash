//! Persistent agent_test_limits records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "registry", table_name = "agent_test_limits")]
#[derive(Serialize, Deserialize)]
pub struct AgentTestLimit {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(default = 20000)]
	pub max_input_bytes: i32,
	#[field(default = 2048)]
	pub max_output_tokens: i32,
	#[field(default = 16384)]
	pub max_total_tokens: i32,
	#[field(default = 12)]
	pub max_steps: i32,
	#[field(default = 60)]
	pub max_duration_secs: i32,
	#[field(default = 2)]
	pub max_concurrent: i32,
	#[field(default = 30)]
	pub payload_days: i32,
	#[field(default = 90)]
	pub incident_evidence_days: i32,
}
