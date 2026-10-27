//! Admission shared by registration and each inference dispatch.
use crate::{Error, Result, provider_access::Context};
use aidash_domain::{model::ModelConfig, tool::provider_tools::PinnedTool};

/// Integration seam for #542: Data Protection connects its effective Tenant /
/// delegation rule here. W4 deliberately defines no parallel protection model.
pub fn admit_data_protection() -> Result<()> {
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
	admit_data_protection()?;
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
