//! API response contracts.
use crate::store::Invocation;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, schemars::JsonSchema)]
pub struct RunDetails {
	/// Original provider-owned activity, newest first, paged by inspection offset.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub provider_tool_activity: Vec<aidash_domain::tool::provider_tools::Activity>,
	/// Metadata-only gateway requests, paged with the inspection offset.
	#[serde(default)]
	pub egress_requests: Vec<aidash_domain::egress::RequestRecord>,
	/// Awake stretches of this execution, in start order.
	#[serde(default)]
	pub stints: Vec<aidash_domain::model_call::Stint>,
	pub run: crate::domain::RunInspection,
	/// The tool call a paused Run stopped on, present only when the caller may decline it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub declinable_call: Option<aidash_domain::run_state::DeclinableCall>,
	pub invocations: Vec<Invocation>,
	pub memory: Option<aidash_domain::memory::Binding>,
	/// Each current route lists MIME types accepted together by the run model.
	pub media_input_routes: Vec<Vec<String>>,
}

/// The pages one input asked a Run to read.
#[derive(Serialize, JsonSchema)]
pub struct RunUrlIntent {
	/// `task` for the input that started or queued the Run, `run_message`
	/// for a later message steering it.
	pub input: UrlIntentInput,
	pub pages: Vec<String>,
	pub requested_by: String,
	pub requested_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UrlIntentInput {
	Task,
	RunMessage,
}
