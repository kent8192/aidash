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
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct RemoteExecutionStatus {
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
			grant: row.grant,
			execution: row.execution.map(Into::into),
			unavailable: row.unavailable,
			semantic: row.semantic,
		}
	}
}
