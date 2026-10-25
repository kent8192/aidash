//! A sandbox request pins only authorized inputs and commits its concurrency admission before driving work.
use super::{
	deferred::{self, Deferred},
	execution::{Job, SessionExposure},
	plugins,
};
use crate::{
	Error, Result,
	plugin_tools::{Listings, pin_listed},
	ports::{
		registry::workbench::sandbox::admission::{AdmissionRepository, SandboxModels},
		tools::PluginSessions,
	},
};
use aidash_domain::{
	agent_plugin::PluginTools,
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
use std::sync::Arc;
use uuid::Uuid;
pub struct Admission<'a> {
	pub repository: &'a dyn AdmissionRepository,
	pub models: &'a dyn SandboxModels,
	/// Lists the Agent's remote plugin servers once when a test starts, so their tools can be
	/// offered. Without it those tools are absent and the test says so.
	pub plugin_sessions: Option<Arc<dyn PluginSessions>>,
}
pub struct Admitted {
	pub session: TestSession,
	pub job: Job,
}
enum Phase {
	ListingReservation,
	Complete,
}
/// Attachment text is already pinned by the admitted Source. Mounted roots have
/// no live execution area in Workbench: their contents must be explicit fixtures.
fn skill_source_context(
	config: &AgentConfig,
	snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
	input: &TestInput,
) -> Result<String> {
	let mut text = String::new();
	for attachment in &config.skill_attachments {
		let metadata = aidash_domain::capabilities::skills::validate(attachment)?;
		text.push_str("\nPinned Skill Source:\n");
		text.push_str(&serde_json::to_string(&metadata)?);
		text.push('\n');
		text.push_str(&attachment.instructions);
	}
	if !config.skill_roots.is_empty() {
		let missing = || {
			Error::Invalid("mounted Skill Sources require successful skill_list and skill_load fixtures for sandbox context".into())
		};
		let fixture = |operation| -> Result<&aidash_domain::registry::workbench::sandbox::Fixture> {
			let binding = snapshot.operation(operation)?;
			let value = input
				.fixtures
				.get(binding.alias.as_deref().ok_or_else(missing)?)
				.ok_or_else(missing)?;
			if !matches!(value.status, sandbox::FixtureStatus::Success) {
				return Err(missing());
			}
			Ok(value)
		};
		let list = fixture("skill_list")?;
		let load = fixture("skill_load")?;
		let metadata: aidash_domain::capabilities::skills::SkillMetadata =
			serde_json::from_value(load.response["skill"].clone()).map_err(|_| missing())?;
		let listed = list.response["skills"].as_array().ok_or_else(missing)?;
		if !listed.iter().any(|value| value == &load.response["skill"])
			|| !config
				.skill_roots
				.iter()
				.any(|root| metadata.origin.contains(&format!(":{root}/")))
			|| load.response["path"] != "SKILL.md"
			|| load.response["truncated"] != false
		{
			return Err(missing());
		}
		let instructions = load.response["content"]
			.as_str()
			.filter(|text| !text.trim().is_empty())
			.ok_or_else(missing)?;
		text.push_str("\nMounted Skill Source (sandbox fixtures):\n");
		text.push_str(&serde_json::to_string(&metadata)?);
		text.push('\n');
		text.push_str(instructions);
	}
	Ok(text)
}

pub fn admit<'a>(
	admission: &'a Admission<'_>,
	id: Uuid,
	input: TestInput,
) -> futures_util::future::BoxFuture<'a, Result<Admitted>> {
	// Check the transport/admission future's Send bound once at its owner, rather than expanding
	// its two transaction phases through every native HTTP handler's coroutine obligations.
	Box::pin(async move {
		sandbox::validate_request(&input)?;
		let continuation = input.continue_from.is_some();
		let phase = if admission.plugin_sessions.is_some() && !continuation {
			Phase::ListingReservation
		} else {
			Phase::Complete
		};
		// Validate every admission constraint and reserve the Tenant slot before any network I/O.
		let reserved = prepare(admission, id, input, &Listings::new(), None, phase).await?;
		let Some(sessions) = admission.plugin_sessions.as_ref().filter(|_| !continuation) else {
			return Ok(reserved);
		};
		let Some(agent) = plugins::remote_plugins(&reserved.job.pinned_draft) else {
			return Ok(reserved);
		};
		let listed = plugins::list(sessions, &agent).await;
		let session_id = reserved.session.id;
		let conversation = reserved.session.conversation.clone();
		let result = prepare(
			admission,
			id,
			reserved.job.input,
			&listed,
			Some(&reserved.session),
			Phase::Complete,
		)
		.await;
		if result.is_err() {
			// The second scope has been dropped, so settlement never waits on its own locks.
			admission
				.repository
				.finish(
					session_id,
					&sandbox::TestOutcome {
						status: "failed".into(),
						conversation: conversation.unwrap_or_else(|| json!([])),
						tool_calls: json!([]),
						usage: json!({}),
						error: Some("plugin listing admission failed".into()),
					},
				)
				.await?;
		}
		result
	})
}

/// With a reservation, recheck its authority and pinned dependencies, then complete the same slot.
async fn prepare(
	admission: &Admission<'_>,
	id: Uuid,
	input: TestInput,
	listed: &Listings,
	reserved: Option<&TestSession>,
	phase: Phase,
) -> Result<Admitted> {
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
	let snapshot = scope.bindings(&draft, &entry).await?;
	let config = AgentConfig::from_snapshot(&snapshot)?;
	let model = scope.effective(&config.model).await?;
	let model_config: ModelConfig = serde_json::from_value(model.config)?;
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
		Some(ProfilePin {
			id: value.id,
			revision: value.revision,
			tenant: draft.tenant.clone(),
			rules,
		})
	} else {
		None
	};
	let (mut conversation, continued_tools) = if let Some(previous_id) = input.continue_from {
		let previous = scope.session(previous_id, false).await?;
		let tools = previous.scenario.get("plugin_tools").cloned();
		let conversation = sandbox::continued_conversation(
			previous,
			&draft,
			&input,
			profile.as_ref().map(|pin| pin.revision),
		)?;
		(conversation, tools)
	} else {
		(Vec::new(), None)
	};
	conversation.push(json!({"role":"user","content":input.message}));
	// A continued session keeps what its first request pinned and never lists again.
	let plugin_tools = match continued_tools {
		Some(saved) => serde_json::from_value(saved)?,
		None if matches!(phase, Phase::ListingReservation)
			&& plugins::remote_plugins(&draft).is_some() =>
		{
			// Listing has not answered yet; do not charge synthetic unavailable warnings.
			PluginTools::default()
		}
		None => pin_listed(&snapshot, listed)?,
	};
	let tool_references = snapshot
		.bindings
		.iter()
		.filter(|b| b.excluded_reason.is_none())
		.filter_map(|b| {
			b.alias
				.as_ref()
				.map(|alias| (alias.clone(), b.identity.local()))
		})
		.collect();
	let mut instructions = aidash_domain::context::agent_instructions("");
	if input.mode == "real" {
		instructions.push_str("\n\nSandbox: only tools in the selected test connection profile can reach its isolated endpoint. Other tools need an explicit fixture; never claim an unprovided result.\n");
	} else {
		instructions.push_str("\n\nSandbox: all tool calls are simulated from explicit fixtures. Never claim an unprovided tool result.\n");
	}
	if let Some(note) = plugins::note(&plugin_tools) {
		instructions.push_str(&note);
	}
	let (tool_specs, exposure) = if let Some(discoverable) =
		Deferred::new(&snapshot, Some(&plugin_tools))?
	{
		let prefix = instructions.clone();
		// A continued session resumes the Exposure set it ended with.
		let state = deferred::replayed(&conversation)?;
		let (text, tools) = discoverable.request(&snapshot, &state)?;
		instructions.push_str(&text);
		(tools, Some((prefix, state)))
	} else {
		for skill in &config.skills {
			let skill = scope.effective(skill).await?;
			instructions.push_str("\nSkill:\n");
			instructions.push_str(&aidash_domain::registry::rules::skill_instructions(&skill)?);
		}
		instructions.push_str(&skill_source_context(&config, &snapshot, &input)?);
		let mut specs: Vec<_> = snapshot
			.bindings
			.iter()
			.filter(|b| b.excluded_reason.is_none())
			.filter_map(|b| {
				b.alias
					.as_deref()
					.map(|alias| crate::tools::plugin_specification(&b.definition, alias))
			})
			.collect();
		specs.extend(
			crate::tools::agent_plugin::pinned_specifications(Some(&plugin_tools)).into_values(),
		);
		(specs, None)
	};
	let suffix_start = instructions.len();
	instructions.push_str("\nAdditional instructions:\n");
	instructions.push_str(&config.instructions);
	let exposure = exposure.map(|(prefix, state)| SessionExposure {
		prefix,
		suffix: instructions[suffix_start..].to_owned(),
		state,
	});
	let mut request = ModelRequest {
		content_parts: Vec::new(),
		instructions,
		context: json!({"test_message":input.message,"private_references":draft.documents,"test_mode":input.mode,"profile_id":input.profile_id}).into(),
		tools: tool_specs,
		max_output_tokens: (limits.max_output_tokens as u32).min(model_config.output_token_limit()),
		response_format: None,
		cache_scope: None,
		cache_breakpoints: false,
		disable_provider_transforms: false,
		input_format: Default::default(),
		estimate: Default::default(),
	};
	if input.continue_from.is_some()
		&& let Some(context) = request.context.legacy_mut()
	{
		context["conversation"] = json!(conversation);
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
	let scenario = json!({"mode":input.mode,"profile_id":input.profile_id,"profile_revision":profile.as_ref().map(|value|value.revision),"continue_from":input.continue_from,"fixtures":fixtures,"binding_snapshot":snapshot,"plugin_tools":plugin_tools});
	// Keep draft, policy, credential, and tenant-concurrency locks until each phase commits.
	let session = if let Some(reserved) = reserved {
		let current = scope.session(reserved.id, true).await?;
		if current.status != "running"
			|| current.draft_id != draft.id
			|| current.tenant != draft.tenant
			|| current.revision != draft.revision
			|| current.scenario["binding_snapshot"] != scenario["binding_snapshot"]
			|| current.scenario["profile_revision"] != scenario["profile_revision"]
		{
			return Err(Error::Conflict(
				"test admission changed during plugin listing".into(),
			));
		}
		scope.complete_listing(reserved.id, scenario).await?
	} else {
		scope
			.admit(&draft, &limits, scenario, json!(conversation))
			.await?
	};
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
			plugin_tools,
			exposure,
		},
	})
}
#[cfg(test)]
mod tests;
