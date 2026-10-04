//! Model-backed sandbox. Tool calls are recorded and resolved exclusively from
//! explicit fixtures; no runtime tool executor is reachable from this module.
use super::*;
use crate::apps::registry::workbench::models::{
	AgentDraft, AgentTestLimit, AgentTestProfile, AgentTestSession,
};
use crate::{
	provider::{ModelRequest, provider},
	registry::{EntityRef, ModelConfig},
};
use reinhardt::db::backends::{
	DatabaseConnection as BackendConnection, PostgresBackend, TransactionExecutor,
	dialect::postgres::PgTransactionExecutor,
};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::injectable;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
struct ProfilePin {
	id: String,
	revision: i64,
	tenant: String,
	rules: Vec<profile::RealToolRule>,
	credential_fingerprints: BTreeMap<String, Vec<u8>>,
}

struct TestJob {
	input: TestInput,
	actor: Actor,
	profile: Option<ProfilePin>,
	tool_references: Vec<EntityRef>,
	limits: TestLimits,
	context_window: usize,
	agent_max_steps: i32,
	model_provider: std::sync::Arc<dyn crate::provider::ModelProvider>,
	request: ModelRequest,
	initial_conversation: Vec<Value>,
	pinned_draft: Draft,
	model_credential: Option<(String, Vec<u8>)>,
}

fn credential_fingerprint(name: &str) -> Result<Vec<u8>> {
	Ok(Sha256::digest(crate::config::secret(name)?.as_bytes()).to_vec())
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

async fn complete(f: Federation, session_id: Uuid, job: TestJob) -> Result<()> {
	let outcome = tokio::time::timeout(
		std::time::Duration::from_secs(job.limits.max_duration_secs as u64),
		simulate(&f, session_id, &job),
	)
	.await;
	let (status, conversation, calls, usage, error) = match outcome {
		Ok(Ok(result)) => result,
		Ok(Err(error)) => {
			let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
			let prior = load_session(&mut tx, session_id).await?;
			Box::new(tx).commit().await?;
			(
				if has_unknown_call(&prior.tool_calls) {
					"outcome_unknown"
				} else {
					"failed"
				},
				prior
					.conversation
					.unwrap_or_else(|| json!([{"role":"user","content":job.input.message}])),
				prior.tool_calls.unwrap_or_else(|| json!([])),
				prior.usage,
				Some(error.to_string()),
			)
		}
		Err(_) => {
			let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
			let prior = load_session(&mut tx, session_id).await?;
			Box::new(tx).commit().await?;
			let dispatched = has_unknown_call(&prior.tool_calls);
			(
				if dispatched {
					"outcome_unknown"
				} else {
					"timed_out"
				},
				prior
					.conversation
					.unwrap_or_else(|| json!([{"role":"user","content":job.input.message}])),
				prior.tool_calls.unwrap_or_else(|| json!([])),
				prior.usage,
				Some(if dispatched {
					"test timed out while an external call was in flight; its outcome is unknown"
						.into()
				} else {
					"model request timed out; provider outcome is unknown".into()
				}),
			)
		}
	};
	let lease = f.store.orm_connection()?;
	lease
		.handle()
		.atomic(async |tx| {
			AgentTestSession::finish(tx, session_id, status, conversation, calls, usage, error)
				.await
		})
		.await?;
	Ok(())
}

type SimulationResult = (&'static str, Value, Value, Value, Option<String>);

fn has_unknown_call(calls: &Option<Value>) -> bool {
	calls
		.as_ref()
		.and_then(Value::as_array)
		.is_some_and(|items| {
			items
				.iter()
				.any(|item| item["outcome"] == "outcome_unknown")
		})
}

async fn prepare_real_dispatch(
	tx: &mut dyn TransactionExecutor,
	session_id: Uuid,
	actor: &Actor,
	pin: &ProfilePin,
	rule: &profile::RealToolRule,
	call: &crate::provider::ToolCall,
) -> Result<(TestSession, reqwest::RequestBuilder)> {
	let session = AgentTestSession::read(tx, session_id, true).await?;
	if session.status != "running" {
		return Err(Error::Conflict("test was stopped".into()));
	}
	if let Actor::Subject(identity) = actor {
		identity.lock_native(tx, false).await?;
	}
	let draft = AgentDraft::read(tx, session.draft_id, true).await?;
	if draft.archived || draft.revision != session.revision {
		return Err(Error::Conflict(
			"draft revision changed or is archived".into(),
		));
	}
	authorize(tx, actor, &draft, "agent_draft.test", true).await?;
	let current = AgentTestProfile::locked(tx, &pin.tenant, &pin.id).await?;
	if !current.enabled
		|| current.revision != pin.revision
		|| current.rules != serde_json::to_value(&pin.rules)?
	{
		return Err(Error::Conflict(
			"test profile changed or was disabled".into(),
		));
	}
	for (name, fingerprint) in &pin.credential_fingerprints {
		if credential_fingerprint(name)? != *fingerprint {
			return Err(Error::Conflict("test credential changed".into()));
		}
	}
	let tool = admission::effective(tx, &rule.tool.id, &rule.tool.version).await?;
	profile::validate_real_rule(rule, &tool)?;
	jsonschema::validator_for(&tool.schema)
		.map_err(|e| Error::Invalid(e.to_string()))?
		.validate(&call.arguments)
		.map_err(|e| Error::Invalid(e.to_string()))?;
	let client = reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.timeout(Duration::from_secs(30))
		.build()?;
	let mut request = client
		.post(&rule.endpoint)
		.header("idempotency-key", format!("test-{session_id}-{}", call.id))
		.json(&call.arguments);
	if let Some(name) = &rule.credential_env {
		request = request.bearer_auth(crate::config::secret(name)?);
	}
	Ok((session, request))
}

async fn record_pre_dispatch_denial(
	f: &Federation,
	session_id: Uuid,
	call: &crate::provider::ToolCall,
	error: &Error,
) -> Result<()> {
	let lease = f.store.orm_connection()?;
	lease
		.handle()
		.atomic(async |tx| {
			let session = AgentTestSession::read(tx, session_id, true).await?;
			let mut recorded = session.tool_calls.unwrap_or_else(|| json!([]));
			if let Some(pending) = recorded
				.as_array_mut()
				.and_then(|calls| calls.last_mut())
				.filter(|pending| {
					pending["id"] == call.id && pending["outcome"] == "outcome_unknown"
				}) {
				pending["outcome"] = json!("denied");
				pending["error"] = json!(error.to_string());
				AgentTestSession::calls(tx, session_id, recorded, false).await?;
			}
			Ok(())
		})
		.await
}

async fn invoke_real(
	f: &Federation,
	session_id: Uuid,
	actor: &Actor,
	pin: &ProfilePin,
	rule: &profile::RealToolRule,
	call: &crate::provider::ToolCall,
) -> Result<(Value, &'static str)> {
	let action = call.arguments["action"]
		.as_str()
		.ok_or_else(|| Error::Invalid("real Tool arguments require an action".into()))?;
	let resource = call.arguments["resource"]
		.as_str()
		.ok_or_else(|| Error::Invalid("real Tool arguments require a resource".into()))?;
	if !rule.allowed_actions.iter().any(|allowed| allowed == action)
		|| !rule
			.allowed_resources
			.iter()
			.any(|allowed| allowed == resource)
	{
		return Err(Error::Forbidden);
	}
	// Check every dispatch prerequisite before committing pending evidence. The
	// durable marker still precedes network I/O, so a crash after dispatch cannot
	// erase an external effect or cause the worker to retry it.
	let pending = json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"outcome_unknown","endpoint":rule.endpoint});
	let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
	let (session, _) = prepare_real_dispatch(&mut tx, session_id, actor, pin, rule, call).await?;
	let mut calls = session.tool_calls.unwrap_or_else(|| json!([]));
	calls
		.as_array_mut()
		.ok_or_else(|| Error::Invalid("invalid test call log".into()))?
		.push(pending);
	let mut native = tx;
	AgentTestSession::calls(&mut native, session_id, calls, true).await?;
	Box::new(native).commit().await?;

	// Hold the exact session, identity, policy, draft and profile authority
	// through dispatch. Stop/revocation/profile changes wait for this boundary.
	let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
	let (_, request) =
		match prepare_real_dispatch(&mut tx, session_id, actor, pin, rule, call).await {
			Ok(prepared) => prepared,
			Err(error) => {
				Box::new(tx).rollback().await?;
				record_pre_dispatch_denial(f, session_id, call, &error).await?;
				return Err(error);
			}
		};
	let response = request.send().await?;
	let (result, outcome) = if response.status().is_success() {
		(crate::response::json(response, 256_000).await?, "real")
	} else {
		(
			json!({"error":"test Tool returned an HTTP error","status":response.status().as_u16()}),
			"failed",
		)
	};
	let mut recorded = load_session(&mut tx, session_id)
		.await?
		.tool_calls
		.ok_or_else(|| Error::Conflict("pending test Tool call was lost".into()))?;
	let last = recorded
		.as_array_mut()
		.and_then(|calls| calls.last_mut())
		.ok_or_else(|| Error::Conflict("pending test Tool call was lost".into()))?;
	if last["id"] != call.id || last["outcome"] != "outcome_unknown" {
		return Err(Error::Conflict("pending test Tool call changed".into()));
	}
	*last = json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":outcome,"result":result,"endpoint":rule.endpoint});
	let mut native = tx;
	AgentTestSession::calls(&mut native, session_id, recorded, false).await?;
	Box::new(native).commit().await?;
	Ok((result, outcome))
}

async fn simulate(f: &Federation, session_id: Uuid, job: &TestJob) -> Result<SimulationResult> {
	let TestJob {
		input,
		actor,
		profile,
		tool_references,
		limits,
		context_window,
		agent_max_steps,
		model_provider,
		..
	} = job;
	let mut request = job.request.clone();
	let mut conversation = job.initial_conversation.clone();
	let mut calls = Vec::new();
	let mut input_tokens = 0_u64;
	let mut output_tokens = 0_u64;
	let mut usage_complete = true;
	let mut status = "blocked";
	let mut error = None;
	for _ in 0..limits.max_steps.min(*agent_max_steps) {
		let lease = f.store.orm_connection()?;
		let still_running = lease
			.handle()
			.atomic(async |tx| {
				Ok::<_, Error>(
					AgentTestSession::read(tx, session_id, false).await?.status == "running",
				)
			})
			.await?;
		if !still_running {
			status = "stopped";
			break;
		}
		if let Some((name, fingerprint)) = &job.model_credential
			&& credential_fingerprint(name)? != *fingerprint
		{
			return Err(Error::Conflict(
				"model credential changed during test".into(),
			));
		}
		let mut authority = PgTransactionExecutor::new(f.store.pool.begin().await?);
		if let Actor::Subject(identity) = actor {
			identity.lock_native(&mut authority, false).await?;
		}
		let current_draft = AgentDraft::read(&mut authority, job.pinned_draft.id, false).await?;
		authorize(
			&mut authority,
			actor,
			&current_draft,
			"agent_draft.test",
			true,
		)
		.await?;
		validate_content(f, &job.pinned_draft, actor, &mut authority).await?;
		if let Some(pin) = profile {
			let current = AgentTestProfile::locked(&mut authority, &pin.tenant, &pin.id).await?;
			if !current.enabled
				|| current.revision != pin.revision
				|| current.rules != serde_json::to_value(&pin.rules)?
			{
				return Err(Error::Conflict(
					"test profile changed during session".into(),
				));
			}
		}
		Box::new(authority).commit().await?;
		if request.input_body().to_string().len() > limits.max_input_bytes as usize
			|| request.estimated_total_tokens() > *context_window
			|| input_tokens
				.saturating_add(output_tokens)
				.saturating_add(request.estimated_total_tokens() as u64)
				> limits.max_total_tokens as u64
		{
			error = Some("test context exceeds configured input or model window limit".into());
			break;
		}
		let response = model_provider.infer(request.clone()).await?;
		input_tokens = input_tokens.saturating_add(response.input_tokens);
		output_tokens = output_tokens.saturating_add(response.output_tokens);
		usage_complete &= response.usage_complete;
		if !response.usage_complete {
			error =
				Some("provider usage is incomplete; test token limits cannot be verified".into());
			break;
		}
		if output_tokens > limits.max_output_tokens as u64 {
			error = Some("test output token limit exceeded by model response".into());
			break;
		}
		if input_tokens.saturating_add(output_tokens) > limits.max_total_tokens as u64 {
			error = Some("test total token limit exceeded by model response".into());
			break;
		}
		conversation.push(
			json!({"role":"assistant","content":response.text,"tool_calls":response.tool_calls}),
		);
		if response.tool_calls.is_empty() {
			status = "completed";
			break;
		}
		let mut missing = false;
		for call in response.tool_calls {
			if calls.len() >= limits.max_steps as usize {
				error = Some("test step limit reached".into());
				missing = true;
				break;
			}
			let real_rule = profile.as_ref().and_then(|pin| {
				call.name
					.strip_prefix("plugin_")
					.and_then(|index| index.parse::<usize>().ok())
					.and_then(|index| {
						request
							.tools
							.iter()
							.find(|tool| tool.name == call.name)
							.map(|_| index)
					})
					.and_then(|index| tool_references.get(index))
					.and_then(|selected| pin.rules.iter().find(|rule| selected == &rule.tool))
			});
			let fixture = input.fixtures.get(&call.name);
			let result = if let Some(rule) = real_rule {
				match invoke_real(
					f,
					session_id,
					actor,
					profile.as_ref().expect("real rule requires profile"),
					rule,
					&call,
				)
				.await
				{
					Ok((output, outcome)) => {
						json!({"id":call.id,"name":call.name,"arguments":call.arguments,"result":output,"outcome":outcome})
					}
					Err(reason) => {
						missing = true;
						let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
						let pending = load_session(&mut tx, session_id).await?.tool_calls;
						Box::new(tx).commit().await?;
						let dispatched = has_unknown_call(&pending);
						status = if dispatched {
							"outcome_unknown"
						} else {
							"blocked"
						};
						error = Some(reason.to_string());
						json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":if dispatched { "outcome_unknown" } else { "denied" },"error":reason.to_string()})
					}
				}
			} else {
				if fixture.is_none() {
					missing = true;
				}
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"fixture":fixture,"outcome":if fixture.is_none() { "missing_fixture" } else { "simulated" }})
			};
			conversation.push(json!({"role":"tool","content":result}));
			calls.push(result);
			let lease = f.store.orm_connection()?;
			lease.handle().atomic(async |tx| AgentTestSession::progress(tx, session_id, json!(conversation), json!(calls), json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete})).await).await?;
		}
		if missing {
			if error.is_none() {
				error = Some("a tool call has no explicit simulated fixture".into());
			}
			break;
		}
		if output_tokens >= limits.max_output_tokens as u64 {
			error = Some("test output token limit reached".into());
			break;
		}
		request.max_output_tokens =
			(limits.max_output_tokens as u64 - output_tokens).min(u32::MAX as u64) as u32;
		request.context["conversation"] = json!(conversation);
	}
	if status == "blocked" && error.is_none() {
		error = Some("test step limit reached".into());
	}
	Ok((
		status,
		json!(conversation),
		json!(calls),
		json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete}),
		error,
	))
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
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, false).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.test", true).await?;
		let value = AgentTestLimit::locked(&mut tx, &draft.tenant).await?;
		Box::new(tx).commit().await?;
		Ok(value)
	}
	pub(crate) async fn set_limits(
		&self,
		actor: Actor,
		tenant: String,
		input: TestLimits,
	) -> Result<TestLimits> {
		let f = self.runtime.clone();
		if !matches!(actor, Actor::Operator) {
			return Err(Error::Forbidden);
		}
		if tenant != input.tenant {
			return Err(Error::Invalid("tenant mismatch".into()));
		}
		validate_limits(&input)?;
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		AgentTestLimit::save(&mut tx, &input).await?;
		Box::new(tx).commit().await?;
		Ok(input)
	}
	pub(crate) async fn sessions(&self, actor: Actor, id: Uuid) -> Result<Vec<TestSession>> {
		let f = self.runtime.clone();
		purge_expired(&f.store.pool).await?;
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, false).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await?;
		let mut native = tx;
		let rows = AgentTestSession::page(&mut native, id).await?;
		Box::new(native).commit().await?;
		Ok(rows)
	}
	pub(crate) async fn stop(&self, actor: Actor, id: Uuid) -> Result<TestSession> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let session = load_session(&mut tx, id).await?;
		let draft = AgentDraft::read(&mut tx, session.draft_id, false).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.test", true).await?;
		let mut native = tx;
		let result = AgentTestSession::stop(&mut native, id).await?;
		Box::new(native).commit().await?;
		Ok(result)
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
				TestJob {
					input,
					actor,
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
			)
			.await
			{
				tracing::error!(%session_id, %error, "sandbox session completion failed");
			}
		});
		Ok(session)
	}
}
