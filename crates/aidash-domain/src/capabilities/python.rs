//! A Python heap identity acknowledges one exact authority, version, mount and runner.
use crate::RunMetadata;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Python {
	pub idempotency_key: Uuid,
	pub code: String,
	pub expected_revision: i64,
	pub expected_session_id: Option<Uuid>,
	pub timeout_seconds: Option<u64>,
}
pub fn fingerprint(
	run: &RunMetadata,
	subjects: &[String],
	credential: Uuid,
	policy_revision: i64,
	generation: i64,
	image: Option<&str>,
) -> String {
	crate::registry::rules::digest(
		&json!({"agent":[run.home_node,run.agent_id,run.agent_version],"subjects":subjects,"credential":credential,"policy_revision":policy_revision,"area_generation":generation,"image":image}),
	)
}
