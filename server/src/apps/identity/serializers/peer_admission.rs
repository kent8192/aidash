use serde::{Deserialize, Serialize};
// Serializable contracts for authorization.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;

#[derive(Deserialize, JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
	pub(crate) grant_id: Uuid,
}

#[derive(Serialize, Deserialize, JsonSchema, reinhardt::Validate)]
pub(crate) struct Admission {
	pub(crate) id: Uuid,
	pub(crate) source_node: String,
	pub(crate) grant_id: Uuid,
	pub(crate) task_id: Uuid,
	pub(crate) expires_at: DateTime<Utc>,
}

// Serializable peer admission contracts.

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteExecutionControlInput {
	pub(crate) grant_id: Uuid,
	pub(crate) action: crate::authorization::remote::execution::RemoteExecutionControl,
}

#[derive(Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageInput {
	pub(crate) grant_id: Uuid,
	pub(crate) message: crate::authorization::remote::execution::RemoteExecutionMessageInput,
}

use uuid::Uuid;

/// Canonical exact recovery request, shared by routing and receiving-key CAS.
/// The ordinary HTTP/local handlers parse this same deny-unknown-fields type.
pub(crate) fn cancel_request(
	request: &aidash_application::federation::inbound::RequestDigest<'_>,
) -> Option<(Uuid, Uuid)> {
	if request.method != "POST" || request.path.contains('?') {
		return None;
	}
	let id = request
		.path
		.strip_prefix("/scoped/execution/admissions/")?
		.strip_suffix("/control")?;
	let admission = Uuid::parse_str(id).ok()?;
	if admission.to_string() != id {
		return None;
	}
	let input: RemoteExecutionControlInput = serde_json::from_slice(request.body).ok()?;
	if !matches!(
		input.action,
		aidash_domain::federation::execution::admission::RemoteExecutionControl::Cancel
	) {
		return None;
	}
	Some((admission, input.grant_id))
}
