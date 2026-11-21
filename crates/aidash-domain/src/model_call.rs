//! Ownership and prompt-free evidence for model requests outside Run steps.
use crate::provider::{ModelRequest, usage::ReportedUsage};
use crate::registry::{EntityRef, rules::digest};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ModelCallPurpose")]
pub enum Purpose {
	CompactionSummary,
	MemoryRole,
	WorkbenchTest,
	MediaCheck,
	McpSampling,
}

impl Purpose {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::CompactionSummary => "compaction_summary",
			Self::MemoryRole => "memory_role",
			Self::WorkbenchTest => "workbench_test",
			Self::MediaCheck => "media_check",
			Self::McpSampling => "mcp_sampling",
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "ModelCallOwner")]
pub enum Owner {
	Stint { stint_id: Uuid, run_id: Uuid },
	Node { tenant: String, node: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ModelCallOutcome")]
pub enum Outcome {
	Accepted,
	Failed,
	Refused,
}

impl Outcome {
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Accepted => "accepted",
			Self::Failed => "failed",
			Self::Refused => "refused",
		}
	}
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelCall {
	pub id: Uuid,
	pub purpose: Purpose,
	pub owner: Owner,
	pub model: EntityRef,
	pub request_digest: String,
	pub outcome: Option<Outcome>,
	pub usage: Option<ReportedUsage>,
	pub started_at: DateTime<Utc>,
	pub ended_at: Option<DateTime<Utc>>,
}

/// Evidence ready to attach to an attempt, before any provider I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
	pub purpose: Purpose,
	pub request_digest: String,
}

impl Dispatch {
	pub fn new(purpose: Purpose, request: &ModelRequest) -> crate::Result<Self> {
		let mut body = json!({
			"input_format": request.input_format,
			"body": request.model_input_body()?,
			"max_output_tokens": request.max_output_tokens,
		});
		body.sort_all_objects();
		Ok(Self {
			purpose,
			request_digest: digest(&body),
		})
	}
}

/// One awake stretch and its auxiliary requests in execution inspection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Stint {
	pub id: Uuid,
	pub node: String,
	pub agent_id: String,
	pub agent_version: String,
	pub started_at: DateTime<Utc>,
	pub ended_at: Option<DateTime<Utc>>,
	pub end_reason: Option<String>,
	pub model_calls: Vec<ModelCall>,
}

/// Counts and reported consumption for one purpose outside a Run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "ModelCallUsageByPurpose")]
pub struct UsageByPurpose {
	pub purpose: Purpose,
	pub calls: i64,
	pub input_tokens: Option<i64>,
	pub output_tokens: Option<i64>,
	pub cost_nanocredits: Option<i64>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::provider::{ContentPart, ModelContext};

	#[test]
	fn a_call_has_one_owner_on_the_wire() {
		let id = Uuid::new_v4();
		let owner = Owner::Stint {
			stint_id: id,
			run_id: id,
		};
		let mut encoded = serde_json::to_value(&owner).unwrap();
		assert_eq!(
			serde_json::from_value::<Owner>(encoded.clone()).unwrap(),
			owner
		);
		encoded["tenant"] = json!("acme");
		encoded["node"] = json!("acme");
		assert!(serde_json::from_value::<Owner>(encoded).is_err());
	}

	#[test]
	fn digest_uses_canonical_native_input_and_changes_with_media() {
		let request = ModelRequest {
			instructions: "Summarize".into(),
			context: ModelContext::native_data(&serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap()),
			max_output_tokens: 8,
			..Default::default()
		};
		let dispatch = Dispatch::new(Purpose::CompactionSummary, &request).unwrap();
		assert_eq!(
			dispatch.request_digest,
			"sha256:409c7bd4875c317fd0beb26d1b91b9f439e165968c994c4f0c39c0e030353407"
		);
		let mut request = request;
		request.content_parts = vec![ContentPart::Text("attachment".into())];
		let digest = Dispatch::new(Purpose::CompactionSummary, &request).unwrap();
		assert_ne!(digest.request_digest, dispatch.request_digest);
	}
}
