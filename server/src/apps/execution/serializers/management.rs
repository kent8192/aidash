//! Management API contracts.
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Default, Deserialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunListQuery {
	/// Opaque continuation cursor from the preceding page.
	pub(crate) cursor: Option<String>,
}

#[derive(Default, Deserialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct HumanListQuery {
	pub(crate) run: Option<Uuid>,
	#[serde(default)]
	pub(crate) state: aidash_application::authorization::bounded_reads::HumanState,
	/// Opaque continuation cursor from the preceding page.
	pub(crate) cursor: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ClaimInput {
	pub(crate) revision: i64,
	pub(crate) agent: EntityRef,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ControlInput {
	pub(crate) action: crate::domain::RunControlAction,
}
#[derive(Default, Deserialize, JsonSchema)]
pub(crate) struct EventQuery {
	#[serde(default)]
	pub(crate) after: i64,
	pub(crate) workspace_id: Option<Uuid>,
}
/// Resume after this `progress_seq`; `Last-Event-ID` takes precedence. Without
/// a cursor the stream starts at the Run's latest Inference Attempt.
#[derive(Default, Deserialize, JsonSchema)]
pub(crate) struct InferenceStreamQuery {
	pub(crate) after: Option<i64>,
}
