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
/// What resuming the caller's Runs that an Account suspension paused did to
/// each of them, in Run order (#876).
#[derive(Serialize, JsonSchema)]
pub(crate) struct SuspendedRunsResume {
	pub(crate) runs: Vec<SuspendedRunResume>,
}
/// One Run's result. Each Run is resumed on its own, with the same checks as
/// a resume through `/api/runs/{id}/control`.
#[derive(Serialize, JsonSchema)]
pub(crate) struct SuspendedRunResume {
	pub(crate) run_id: Uuid,
	pub(crate) outcome: SuspendedRunOutcome,
	/// Why the Run was not resumed, as a single resume would answer; absent
	/// when it was resumed.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub(crate) error: Option<String>,
}
/// `resumed`; `blocked` by what the Run read or uses; `refused` by sign-in or
/// authority; `unavailable` while a check cannot be made; `failed` otherwise.
#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SuspendedRunOutcome {
	Resumed,
	Blocked,
	Refused,
	Unavailable,
	Failed,
}
impl SuspendedRunResume {
	pub(crate) fn resumed(run_id: Uuid) -> Self {
		Self {
			run_id,
			outcome: SuspendedRunOutcome::Resumed,
			error: None,
		}
	}
	/// The refusal a single resume of `run_id` answered with.
	pub(crate) fn refused(run_id: Uuid, error: crate::Error) -> Self {
		let blocked = matches!(error, crate::Error::ResumeBlocked(_));
		let response = error.http_response();
		let outcome = match response.status.as_u16() {
			_ if blocked => SuspendedRunOutcome::Blocked,
			401 | 403 => SuspendedRunOutcome::Refused,
			503 => SuspendedRunOutcome::Unavailable,
			_ => SuspendedRunOutcome::Failed,
		};
		let error = serde_json::from_slice::<serde_json::Value>(&response.body)
			.ok()
			.and_then(|body| {
				body.get("error")
					.and_then(|e| e.as_str())
					.map(str::to_owned)
			});
		Self {
			run_id,
			outcome,
			error,
		}
	}
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
