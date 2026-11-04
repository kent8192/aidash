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
pub(crate) struct ScratchList {
	pub node: NodeName,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(crate) struct ScratchLimits {
	pub ceilings: ScratchCeilings,
	pub tenant_hosts: HostAllowlist,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ScratchView {
	#[serde(flatten)]
	space: aidash_domain::scratch::ScratchSpace,
	expired: bool,
}

impl ScratchView {
	pub(crate) fn new(
		space: aidash_domain::scratch::ScratchSpace,
		now: chrono::DateTime<chrono::Utc>,
	) -> Self {
		let expired = space.is_expired(now);
		Self { space, expired }
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use chrono::{Duration, Utc};

	#[test]
	fn scratch_view_reports_idle_expiry() {
		let now = Utc::now();
		let space = aidash_domain::scratch::ScratchSpace {
			id: uuid::Uuid::new_v4(),
			node: aidash_domain::nodes::NodeName::derived("scratch"),
			name: "Project".into(),
			creator: "alice".into(),
			members: Default::default(),
			state: aidash_domain::scratch::ScratchState::Active,
			revision: 0,
			wipe_generation: 0,
			settings: ScratchSettings::default(),
			created_at: now,
			last_used_at: now,
		};
		let value =
			serde_json::to_value(ScratchView::new(space, now + Duration::days(15))).unwrap();
		assert_eq!(value["expired"], true);
		assert_eq!(value["name"], "Project");
		let schema = serde_json::to_value(crate::config::openapi::openapi().unwrap()).unwrap();
		assert_eq!(
			schema["components"]["schemas"]["ScratchView"]["properties"]["expired"]["type"],
			"boolean"
		);
	}
}
