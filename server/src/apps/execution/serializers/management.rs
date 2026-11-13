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
/// The call a Run is paused on, by the key the Run offered it under.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeclineCallInput {
	/// At most 256 characters.
	pub(crate) call: String,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ControlInput {
	pub(crate) action: crate::domain::RunControlAction,
}
impl ControlInput {
	/// Whether `request` starts work again rather than stops it, for the
	/// boundary to decide before the route runs (ADR 0121). A body that does
	/// not parse counts as starting: the route refuses it, but a stop is only
	/// ever a request that says so.
	pub(crate) fn starts_work(request: &reinhardt::Request) -> bool {
		serde_json::from_slice::<Self>(request.body()).map_or(true, |input| {
			!matches!(
				input.action,
				crate::domain::RunControlAction::Pause | crate::domain::RunControlAction::Cancel
			)
		})
	}
}
/// One read of the events, with the rows it could not confirm because a Node
/// did not answer (#770). The cursor to continue from is in a header.
#[derive(Serialize, JsonSchema)]
pub(crate) struct EventPage {
	pub(crate) events: Vec<crate::apps::workspaces::serializers::entities::Event>,
	pub(crate) unconfirmed: Vec<aidash_domain::unconfirmed::Unconfirmed>,
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
