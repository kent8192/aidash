//! Model-backed sandbox. Tool calls are recorded and resolved exclusively from
//! explicit fixtures; no runtime tool executor is reachable from this module.
use super::*;
use crate::apps::registry::workbench::models::{
	AgentDraft, AgentTestLimit, AgentTestProfile, AgentTestSession,
};
use crate::{
	provider::{ModelRequest, provider},
	registry::ModelConfig,
};
use aidash_domain::registry::workbench::sandbox::ProfilePin;
use reinhardt::db::backends::{
	DatabaseConnection as BackendConnection, PostgresBackend, TransactionExecutor,
	dialect::postgres::PgTransactionExecutor,
};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::injectable;
use std::collections::BTreeMap;
use std::sync::Arc;

use aidash_application::registry::workbench::sandbox::execution::Job as TestJob;

fn credential_fingerprint(name: &str) -> Result<Vec<u8>> {
	aidash_application::registry::workbench::sandbox::dispatch::credential_fingerprint(
		crate::bootstrap::workbench_sandbox_credentials().as_ref(),
		name,
	)
	.map_err(Into::into)
}

async fn load_session(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<TestSession> {
	AgentTestSession::read(tx, id, false).await
}

fn validate_limits(value: &TestLimits) -> Result<()> {
	crate::http::validate(value)?;
	if value.max_total_tokens < value.max_output_tokens {
		return Err(Error::Invalid(
			"test limits are outside the supported ranges".into(),
		));
	}
	Ok(())
}

/// Remove ordinary test payloads. The metadata and expiry marker remain.
pub async fn purge_expired(pool: &sqlx::PgPool) -> Result<u64> {
	let lease = DatabaseConnectionLease::register(BackendConnection::new(Arc::new(
		PostgresBackend::new(pool.clone()),
	)))?;
	lease
		.handle()
		.atomic(async |tx| AgentTestSession::purge(tx).await)
		.await
}

async fn complete(f: Federation, session_id: Uuid, actor: Actor, job: TestJob) -> Result<()> {
	aidash_runtime::sandbox::complete(
		crate::bootstrap::workbench_sandbox_execution(&f, actor),
		session_id,
		job,
	)
	.await
	.map_err(Into::into)
}

pub use crate::apps::registry::workbench::serializers::test::{TestInput, TestLimits, TestSession};

#[derive(Clone)]
pub struct BehavioralTests {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_test(#[inject] runtime: Federation) -> BehavioralTests {
	BehavioralTests { runtime }
}

impl BehavioralTests {
	pub(crate) async fn get_limits(&self, actor: Actor, id: Uuid) -> Result<TestLimits> {
		Ok(
			aidash_application::registry::workbench::sandbox::get_limits(
				&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
				id,
			)
			.await?
			.into(),
		)
	}
	pub(crate) async fn set_limits(
		&self,
		actor: Actor,
		tenant: String,
		input: TestLimits,
	) -> Result<TestLimits> {
		Ok(
			aidash_application::registry::workbench::sandbox::set_limits(
				&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
				tenant,
				input.into(),
			)
			.await?
			.into(),
		)
	}
	pub(crate) async fn sessions(&self, actor: Actor, id: Uuid) -> Result<Vec<TestSession>> {
		aidash_application::registry::workbench::sandbox::sessions(
			&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn stop(&self, actor: Actor, id: Uuid) -> Result<TestSession> {
		aidash_application::registry::workbench::sandbox::stop(
			&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn start(
		&self,
		actor: Actor,
		id: Uuid,
		input: TestInput,
	) -> Result<TestSession> {
		let f = self.runtime.clone();
		if input.message.trim().is_empty() || input.fixtures.len() > 64 {
			return Err(Error::Invalid(
				"test requires a message and at most 64 fixtures".into(),
			));
		}
		if !matches!(input.mode.as_str(), "simulated" | "real") {
			return Err(Error::Invalid("test mode must be simulated or real".into()));
		}
		if input.mode == "simulated" && input.profile_id.is_some() {
			return Err(Error::Invalid(
				"simulated tests cannot select a real-tool profile".into(),
			));
		}
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, true).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.test", true).await?;
		if draft.archived || draft.revision != input.expected_revision {
			return Err(Error::Conflict(
				"draft revision changed or is archived".into(),
			));
		}
		let entry = validate_content(&f, &draft, &actor, &mut tx).await?;
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		let model = admission::effective(&mut tx, &config.model.id, &config.model.version).await?;
		let model_config: ModelConfig = serde_json::from_value(model.config)?;
		let model_credential = model_config
			.credential_env
			.as_ref()
			.map(|name| credential_fingerprint(name).map(|digest| (name.clone(), digest)))
			.transpose()?;
		let limits = AgentTestLimit::locked(&mut tx, &draft.tenant).await?;
		validate_limits(&limits)?;
		let profile = if input.mode == "real" {
			let id = input.profile_id.as_deref().ok_or_else(|| {
				Error::Invalid("real tests require an administrator-configured profile".into())
			})?;
			let value = AgentTestProfile::locked(&mut tx, &draft.tenant, id).await?;
			if !value.enabled {
				return Err(Error::Forbidden);
			}
			let rules: Vec<profile::RealToolRule> = serde_json::from_value(value.rules)?;
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
				credential_fingerprints.insert(name.clone(), credential_fingerprint(name)?);
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
			let previous = load_session(&mut tx, previous_id).await?;
			if previous.draft_id != draft.id
				|| previous.revision != draft.revision
				|| previous.status != "completed"
				|| previous.expired_at.is_some()
				|| previous.scenario["mode"] != input.mode
				|| previous.scenario["profile_id"] != serde_json::to_value(&input.profile_id)?
				|| previous.scenario["profile_revision"]
					!= serde_json::to_value(profile.as_ref().map(|pin| pin.revision))?
			{
				return Err(Error::Conflict(
					"previous test context is stale or unavailable; reset the conversation".into(),
				));
			}
			previous
				.conversation
				.and_then(|value| value.as_array().cloned())
				.ok_or_else(|| {
					Error::Conflict("previous test conversation is unavailable".into())
				})?
		} else {
			Vec::new()
		};
		conversation.push(json!({"role":"user","content":input.message}));
		let tool_references = config.tools.clone();
		let mut instructions = crate::context::agent_instructions("");
		if input.mode == "real" {
			instructions.push_str("\n\nSandbox: only tools in the selected test connection profile can reach its isolated endpoint. Other tools need an explicit fixture; never claim an unprovided result.\n");
		} else {
			instructions.push_str("\n\nSandbox: all tool calls are simulated from explicit fixtures. Never claim an unprovided tool result.\n");
		}
		for skill in &config.skills {
			let skill = admission::effective(&mut tx, &skill.id, &skill.version).await?;
			instructions.push_str("\nSkill:\n");
			instructions.push_str(&crate::registry::skill_instructions(&skill)?);
		}
		instructions.push_str("\nAdditional instructions:\n");
		instructions.push_str(&config.instructions);
		let mut tool_specs = crate::tool::builtins()
			.into_iter()
			.filter(|(name, _)| config.permits_builtin(name))
			.map(|(_, tool)| tool.specification())
			.collect::<Vec<_>>();
		for (index, reference) in config.tools.iter().enumerate() {
			let tool = admission::effective(&mut tx, &reference.id, &reference.version).await?;
			if config.allow_task_delegation == Some(false)
				&& matches!(
					serde_json::from_value::<crate::tool::ToolConfig>(tool.config.clone())?,
					crate::tool::ToolConfig::Agent { .. }
				) {
				continue;
			}
			tool_specs.push(crate::tool::plugin_specification(
				&tool,
				&format!("plugin_{index}"),
			));
		}
		let mut request = ModelRequest {
			content_parts: Vec::new(),
			instructions,
			context: json!({"test_message":input.message,"private_references":draft.documents,"test_mode":input.mode,"profile_id":input.profile_id}),
			tools: tool_specs,
			max_output_tokens: (limits.max_output_tokens as u32)
				.min(model_config.output_token_limit()),
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
		let model_provider = provider(f.client.clone(), model_config)?;
		let fixtures = serde_json::to_value(&input.fixtures)?;
		if fixtures.to_string().len() > 65_536 {
			return Err(Error::Invalid("test fixtures exceed 64 KiB".into()));
		}
		// Keep draft, policy, credential, and tenant-concurrency locks until admission commits.
		let mut native = tx;
		let session = AgentTestSession::admit(&mut native, &draft, &limits,
			json!({"mode":input.mode,"profile_id":input.profile_id,"profile_revision":profile.as_ref().map(|value|value.revision),"continue_from":input.continue_from,"fixtures":fixtures}),
			json!(conversation)).await?;
		Box::new(native).commit().await?;
		let session_id = session.id;
		tokio::spawn(async move {
			if let Err(error) = complete(
				f,
				session_id,
				actor,
				TestJob {
					input,
					profile,
					tool_references,
					limits: limits.into(),
					context_window,
					agent_max_steps: config.max_steps,
					model_provider,
					request,
					initial_conversation: conversation,
					pinned_draft: draft.into(),
					model_credential,
				},
			)
			.await
			{
				tracing::error!(%session_id, %error, "sandbox session completion failed");
			}
		});
		Ok(session)
	}
}
