//! Native ORM profile conversion at the storage boundary.
use crate::apps::registry::workbench::models::AgentTestProfile;
pub use aidash_domain::registry::workbench::profile::{
	ProfileInput, ProfileQuery, ProfileSummary, RealToolRule, TestProfile,
};
impl From<AgentTestProfile> for TestProfile {
	fn from(row: AgentTestProfile) -> Self {
		Self {
			tenant: row.tenant,
			id: row.id,
			revision: row.revision,
			enabled: row.enabled,
			rules: row.rules.into_inner(),
			updated_at: row.updated_at,
		}
	}
}
