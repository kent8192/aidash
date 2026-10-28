//! Admission shared by registration and each inference dispatch.
use crate::{Error, Result, provider_access::Context};
use aidash_domain::{
	data_protection::{DataProtection, Destination},
	model::ModelConfig,
	tool::provider_tools::PinnedTool,
};

/// Evaluate the current Tenant/delegation policy supplied by the send adapter.
pub fn admit_data_protection(policy: DataProtection) -> Result<()> {
	policy.permits(Destination::ProviderTool)?;
	Ok(())
}

pub fn admit_request(model: &ModelConfig, context: &Context, tools: &[PinnedTool]) -> Result<()> {
	if tools.is_empty() {
		return Ok(());
	}
	if context.run.is_none() || context.maintenance.is_some() || context.observation.is_some() {
		return Err(Error::Invalid(
			"Provider tools require Run inference".into(),
		));
	}
	admit_data_protection(context.data_protection)?;
	let mut seen = std::collections::BTreeSet::new();
	for tool in tools {
		tool.validate()?;
		aidash_domain::tool::provider_tools::declaration(&tool.tool_type)
			.expect("validated")
			.eligible(model)?;
		if !seen.insert(&tool.tool_type) {
			return Err(Error::Invalid("duplicate Provider tool Binding".into()));
		}
	}
	Ok(())
}
