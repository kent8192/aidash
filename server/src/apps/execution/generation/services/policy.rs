//! Native callers share portable policy validation and persistence adapters.
pub(crate) use crate::apps::execution::generation::repositories::policy::load;
pub use aidash_domain::generation::policy::{
	Compaction, Embedding, Limits, Permissions, Policy, Spec,
};
pub fn validate(
	spec: &Spec,
	bundle: &aidash_domain::policy::PolicyBundle,
) -> crate::Result<aidash_domain::registry::AgentConfig> {
	aidash_application::generation::policy::validate(
		&crate::bootstrap::registry_validation(),
		spec,
		bundle,
	)
	.map_err(Into::into)
}
