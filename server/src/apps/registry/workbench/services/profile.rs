//! Tenant-owned connection profiles for confined real-tool tests (#361).
use super::*;
use reinhardt::injectable;

pub use crate::apps::registry::workbench::serializers::profile::{
	ProfileInput, ProfileQuery, ProfileSummary, RealToolRule, TestProfile,
};

#[derive(Clone)]
pub struct TestProfiles {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_profile(#[inject] runtime: Federation) -> TestProfiles {
	TestProfiles { runtime }
}

impl TestProfiles {
	pub(crate) async fn list(
		&self,
		actor: Actor,
		query: ProfileQuery,
	) -> Result<Vec<ProfileSummary>> {
		aidash_application::registry::workbench::profile::list(
			&crate::bootstrap::workbench_profile_repository(&self.runtime, actor),
			query,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn administered(
		&self,
		actor: Actor,
		tenant: String,
	) -> Result<Vec<TestProfile>> {
		aidash_domain::policy::identifier(&tenant)?;
		aidash_application::registry::workbench::profile::administered(
			&crate::bootstrap::workbench_profile_repository(&self.runtime, actor),
			tenant,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn put(
		&self,
		actor: Actor,
		(tenant, id): (String, String),
		input: ProfileInput,
	) -> Result<TestProfile> {
		aidash_application::registry::workbench::profile::put(
			&crate::bootstrap::workbench_profile_repository(&self.runtime, actor),
			&crate::bootstrap::workbench_profile_configuration(&self.runtime),
			tenant,
			id,
			input,
		)
		.await
		.map_err(Into::into)
	}
}
