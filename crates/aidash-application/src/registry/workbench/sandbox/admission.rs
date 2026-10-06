//! A sandbox request pins only authorized inputs and commits its concurrency admission before driving work.
use super::{dispatch, execution::Job};
use crate::{
	Error, Result,
	ports::{
		Credentials,
		registry::workbench::sandbox::admission::{AdmissionRepository, SandboxModels},
	},
};
use aidash_domain::{
	model::ModelConfig,
	provider::ModelRequest,
	registry::{
		AgentConfig,
		workbench::{
			profile::RealToolRule,
			sandbox::{self, ProfilePin, TestInput, TestSession},
		},
	},
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;
pub struct Admission<'a> {
	pub repository: &'a dyn AdmissionRepository,
	pub credentials: &'a dyn Credentials,
	pub models: &'a dyn SandboxModels,
}
pub struct Admitted {
	pub session: TestSession,
	pub job: Job,
}
pub async fn admit(admission: &Admission<'_>, id: Uuid, input: TestInput) -> Result<Admitted> {
	sandbox::validate_request(&input)?;
	let mut scope = admission.repository.begin_admission().await?;
	let draft = scope.draft(id, true).await?;
	scope
		.authorize_draft(&draft, "agent_draft.test", true)
		.await?;
	if draft.archived || draft.revision != input.expected_revision {
		return Err(Error::Conflict(
			"draft revision changed or is archived".into(),
		));
	}
	let entry = scope.validate_content(&draft).await?;
	let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
	let model = scope.effective(&config.model).await?;
	let model_config: ModelConfig = serde_json::from_value(model.config)?;
	let model_credential = model_config
		.credential_env
		.as_ref()
		.map(|name| {
			dispatch::credential_fingerprint(admission.credentials, name)
				.map(|digest| (name.clone(), digest))
		})
		.transpose()?;
	let limits = scope.limits(&draft.tenant).await?;
	super::validate_limits(admission.repository, &limits)?;
	let profile = if input.mode == "real" {
		let id = input.profile_id.as_deref().ok_or_else(|| {
			Error::Invalid("real tests require an administrator-configured profile".into())
		})?;
		let value = scope.profile(&draft.tenant, id).await?;
		if !value.enabled {
			return Err(Error::Forbidden);
		}
		let rules: Vec<RealToolRule> = serde_json::from_value(value.rules)?;
		if rules.is_empty() {
			return Err(Error::Invalid(
				"real-tool profile has no permitted Tools".into(),
			));
		}
		for rule in &rules {
			if !config.tools.contains(&rule.tool) {
				return Err(Error::Forbidden);
			}
		}
		let mut credential_fingerprints = BTreeMap::new();
		for name in rules.iter().filter_map(|rule| rule.credential_env.as_ref()) {
			credential_fingerprints.insert(
				name.clone(),
				dispatch::credential_fingerprint(admission.credentials, name)?,
			);
		}
		Some(ProfilePin {
			id: value.id,
			revision: value.revision,
			tenant: draft.tenant.clone(),
			rules,
			credential_fingerprints,
		})
	} else {
		None
	};
	let mut conversation = if let Some(previous_id) = input.continue_from {
		let previous = scope.session(previous_id, false).await?;
		sandbox::continued_conversation(
			previous,
			&draft,
			&input,
			profile.as_ref().map(|pin| pin.revision),
		)?
	} else {
		Vec::new()
	};
	conversation.push(json!({"role":"user","content":input.message}));
	let tool_references = config.tools.clone();
	let mut instructions = aidash_domain::context::agent_instructions("");
	if input.mode == "real" {
		instructions.push_str("\n\nSandbox: only tools in the selected test connection profile can reach its isolated endpoint. Other tools need an explicit fixture; never claim an unprovided result.\n");
	} else {
		instructions.push_str("\n\nSandbox: all tool calls are simulated from explicit fixtures. Never claim an unprovided tool result.\n");
	}
	for skill in &config.skills {
		let skill = scope.effective(skill).await?;
		instructions.push_str("\nSkill:\n");
		instructions.push_str(&aidash_domain::registry::rules::skill_instructions(&skill)?);
	}
	instructions.push_str("\nAdditional instructions:\n");
	instructions.push_str(&config.instructions);
	let mut tool_specs = crate::tools::builtins()
		.into_iter()
		.filter(|(name, _)| config.permits_builtin(name))
		.map(|(_, tool)| tool.specification())
		.collect::<Vec<_>>();
	for (index, reference) in config.tools.iter().enumerate() {
		let tool = scope.effective(reference).await?;
		if config.allow_task_delegation == Some(false)
			&& matches!(
				serde_json::from_value::<aidash_domain::tool::ToolConfig>(tool.config.clone())?,
				aidash_domain::tool::ToolConfig::Agent { .. }
			) {
			continue;
		}
		tool_specs.push(crate::tools::plugin_specification(
			&tool,
			&format!("plugin_{index}"),
		));
	}
	let mut request = ModelRequest {
		content_parts: Vec::new(),
		instructions,
		context: json!({"test_message":input.message,"private_references":draft.documents,"test_mode":input.mode,"profile_id":input.profile_id}),
		tools: tool_specs,
		max_output_tokens: (limits.max_output_tokens as u32).min(model_config.output_token_limit()),
	};
	if input.continue_from.is_some() {
		request.context["conversation"] = json!(conversation);
	}
	let serialized_bytes = serde_json::to_vec(&request)?.len();
	if serialized_bytes > limits.max_input_bytes as usize
		|| request.estimated_total_tokens() > model_config.context_window
		|| request.estimated_total_tokens() > limits.max_total_tokens as usize
	{
		return Err(Error::Invalid(
			"test input exceeds configured or model context limits".into(),
		));
	}
	let context_window = model_config.context_window;
	let model_provider = admission.models.provider(model_config)?;
	let fixtures = serde_json::to_value(&input.fixtures)?;
	if fixtures.to_string().len() > 65_536 {
		return Err(Error::Invalid("test fixtures exceed 64 KiB".into()));
	}
	// Keep draft, policy, credential, and tenant-concurrency locks until admission commits.
	let session = scope.admit(&draft,&limits,
			json!({"mode":input.mode,"profile_id":input.profile_id,"profile_revision":profile.as_ref().map(|value|value.revision),"continue_from":input.continue_from,"fixtures":fixtures}),
			json!(conversation)).await?;
	scope.commit().await?;

	Ok(Admitted {
		session,
		job: Job {
			input,
			profile,
			tool_references,
			limits,
			context_window,
			agent_max_steps: config.max_steps,
			model_provider,
			request,
			initial_conversation: conversation,
			pinned_draft: draft,
			model_credential,
		},
	})
}
#[cfg(test)]
mod tests;
