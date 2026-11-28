//! Scratch Space request payloads.
use aidash_domain::{
	nodes::NodeName,
	scratch::{HostAllowlist, ScratchCeilings, ScratchSettings},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchCreate {
	pub node: NodeName,
	pub name: String,
	#[serde(default)]
	pub settings: Option<ScratchSettings>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchMember {
	pub subject: String,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchSettingsUpdate {
	pub revision: i64,
	pub settings: ScratchSettings,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScratchClear {
	/// Omit to list the affected Tasks. Confirm by submitting that exact list.
	pub confirmed_tasks: Option<Vec<uuid::Uuid>>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ScratchClearResult {
	pub sent: bool,
	pub running_tasks: Vec<uuid::Uuid>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ScratchList {
	pub node: NodeName,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ScratchLimits {
	pub ceilings: ScratchCeilings,
	pub tenant_hosts: HostAllowlist,
}

/// Whether one External service on a Scratch Space is usable now (#738).
#[derive(Serialize, JsonSchema)]
pub(crate) struct ExternalServiceStatus {
	pub id: String,
	pub usable: bool,
	/// Reason for an unusable service. Values are `not_found`, `not_approved`, `approval_limited`, and `limits_exceeded`.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub reason: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ScratchView {
	#[serde(flatten)]
	space: aidash_domain::scratch::ScratchSpace,
	/// One entry per External service in the settings, in the settings' order.
	external_service_status: Vec<ExternalServiceStatus>,
}

impl ScratchView {
	pub(crate) fn new(
		space: aidash_domain::scratch::ScratchSpace,
		services: Vec<crate::apps::workspaces::repositories::scratch::ServiceState>,
	) -> Self {
		let external_service_status = services
			.into_iter()
			.map(|state| ExternalServiceStatus {
				id: state.id,
				usable: state.usable,
				reason: state.reason.map(str::to_owned),
			})
			.collect();
		Self {
			space,
			external_service_status,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use chrono::Utc;

	#[test]
	fn scratch_view_reports_space_scope() {
		let now = Utc::now();
		let space = aidash_domain::scratch::ScratchSpace {
			is_default: true,
			id: uuid::Uuid::new_v4(),
			node: aidash_domain::nodes::NodeName::derived("scratch"),
			name: "Node Scratch Space".into(),
			creator: "alice".into(),
			members: Default::default(),
			state: aidash_domain::scratch::ScratchState::Active,
			revision: 0,
			wipe_generation: 0,
			wipe_completed_generation: 0,
			settings: ScratchSettings::default(),
			created_at: now,
			last_used_at: now,
		};
		let value = serde_json::to_value(ScratchView::new(space, Vec::new())).unwrap();
		assert_eq!(value["is_default"], true);
		assert_eq!(value["name"], "Node Scratch Space");
		assert_eq!(value["external_service_status"], serde_json::json!([]));
		let schema = serde_json::to_value(crate::config::openapi::openapi().unwrap()).unwrap();
		assert_eq!(
			schema["components"]["schemas"]["ScratchView"]["properties"]["is_default"]["type"],
			"boolean"
		);
	}
}
