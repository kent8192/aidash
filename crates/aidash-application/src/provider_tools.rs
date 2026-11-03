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

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_domain::{data_protection::Level, model::WireApi, tool::provider_tools};
	#[test]
	fn shell_requires_responses_and_current_standard_policy() {
		let mut model:ModelConfig = serde_json::from_value(serde_json::json!({"provider":"openrouter","provider_credential":"openrouter","model_id":"vendor/model","endpoint":"https://openrouter.ai/api/v1","context_window":100000,"max_output_tokens":1000,"modalities":["text"],"cost":{}})).unwrap();
		let declaration = provider_tools::declaration("openrouter:shell").unwrap();
		let tools = [PinnedTool {
			tool_type: declaration.tool_type.into(),
			binding: declaration.defaults(),
		}];
		let mut context = Context {
			tenant: "acme".into(),
			run: Some(uuid::Uuid::new_v4()),
			..Default::default()
		};
		assert!(admit_request(&model, &context, &tools).is_err());
		model.wire_api = Some(WireApi::Responses);
		admit_request(&model, &context, &tools).unwrap();
		for level in [Level::NoTraining, Level::ZeroDataRetention] {
			context.data_protection.level = level;
			assert!(
				admit_request(&model, &context, &tools)
					.unwrap_err()
					.to_string()
					.contains("Provider tools cannot meet this level")
			);
		}
	}
}
