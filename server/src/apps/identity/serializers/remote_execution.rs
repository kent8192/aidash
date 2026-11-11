use serde::{Deserialize, Serialize};
// Serializable remote execution contracts.
use crate::apps::identity::services::remote::*;

#[derive(reinhardt::Validate, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoteExecutionActivation {
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub run_id: Uuid,
	pub phase: RemoteExecutionPhase,
	pub control: RemoteExecutionControlState,
	pub error: Option<String>,
	#[serde(default)]
	pub semantic_reason: Option<crate::semantic::remote::Failure>,
	/// Typed context-recovery pause reason of the remote Run.
	#[serde(default)]
	pub context_reason: Option<aidash_domain::context::recovery::Failure>,
	/// The tool call the remote Run is paused on, present only when the caller may decline it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub declinable_call: Option<aidash_domain::run_state::DeclinableCall>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct RemoteExecutionStatus {
	pub human_requests: Vec<aidash_domain::HumanRequest>,
	pub grant: Prepared,
	pub execution: Option<RemoteExecutionActivation>,
	pub unavailable: bool,
	pub semantic: crate::semantic::remote::status::Status,
}

pub use aidash_domain::federation::execution::admission::RemoteExecutionControl;

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionControlInput {
	pub action: RemoteExecutionControl,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionDeclineInput {
	/// The key of the call the remote Run is paused on; at most 256 characters.
	#[validate(length(min = 1, max = 256))]
	pub call: String,
	/// Also decline later calls to this tool for the same kind of refusal in this Run.
	#[serde(default)]
	pub rest_of_run: bool,
}

impl RemoteExecutionControlInput {
	/// Whether `request` starts work again rather than stops it, for the
	/// boundary to decide before the route runs (ADR 0121). A body that does
	/// not parse counts as starting: the route refuses it, but a stop is only
	/// ever a request that says so.
	pub fn starts_work(request: &reinhardt::Request) -> bool {
		serde_json::from_slice::<Self>(request.body()).map_or(true, |input| {
			!matches!(
				input.action,
				RemoteExecutionControl::Pause | RemoteExecutionControl::Cancel
			)
		})
	}
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionMessageInput {
	pub id: Uuid,
	#[validate(length(min = 1, max = 64000))]
	pub content: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
pub struct RemoteExecutionMessageReceipt {
	pub id: Uuid,
	pub run_id: Uuid,
	pub accepted: bool,
}

use uuid::Uuid;

use crate::authorization::remote::execution::RemoteExecutionPhase;

use crate::authorization::remote::execution::RemoteExecutionControlState;

impl From<aidash_domain::federation::execution::admission::Activation>
	for RemoteExecutionActivation
{
	fn from(row: aidash_domain::federation::execution::admission::Activation) -> Self {
		Self {
			grant_id: row.grant_id,
			admission_id: row.admission_id,
			run_id: row.run_id,
			phase: row.phase,
			control: row.control,
			error: row.error,
			semantic_reason: row.semantic_reason,
			context_reason: row.context_reason,
			declinable_call: row.declinable_call,
		}
	}
}

impl From<aidash_domain::federation::execution::admission::MessageReceipt>
	for RemoteExecutionMessageReceipt
{
	fn from(row: aidash_domain::federation::execution::admission::MessageReceipt) -> Self {
		Self {
			id: row.id,
			run_id: row.run_id,
			accepted: row.accepted,
		}
	}
}

impl From<RemoteExecutionMessageInput>
	for aidash_domain::federation::execution::admission::Message
{
	fn from(row: RemoteExecutionMessageInput) -> Self {
		Self {
			id: row.id,
			content: row.content,
		}
	}
}
impl From<aidash_domain::federation::execution::home::Status> for RemoteExecutionStatus {
	fn from(row: aidash_domain::federation::execution::home::Status) -> Self {
		Self {
			human_requests: row.human_requests,
			grant: row.grant,
			execution: row.execution.map(Into::into),
			unavailable: row.unavailable,
			semantic: row.semantic,
		}
	}
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteHumanAnswer {
	pub id: Uuid,
	pub response: serde_json::Value,
}
