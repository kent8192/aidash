//! Sandbox reasoning uses explicit fixtures or confined dispatch, never a runtime Tool executor.
use super::{deferred, dispatch};
use crate::{
	Error, Result,
	ports::{
		ModelProvider, NoProgress,
		registry::workbench::{
			profile::ProfileConfiguration,
			sandbox::{
				dispatch::RealToolTransport,
				execution::{ExecutionRepository, TestApproval},
			},
		},
	},
};
use aidash_domain::{
	agent_plugin::PluginTools,
	exposure::ExposureState,
	provider::ModelRequest,
	registry::{
		EntityRef,
		workbench::{
			Draft,
			sandbox::{ProfilePin, TestInput, TestLimits, TestOutcome, has_unknown_call},
		},
	},
};
use serde_json::{Value, json};
use std::sync::Arc;
pub type SessionId = uuid::Uuid;
use uuid::Uuid;
pub struct Execution {
	pub repository: Arc<dyn ExecutionRepository>,
	pub configuration: Arc<dyn ProfileConfiguration>,
	pub transport: Arc<dyn RealToolTransport>,
}
pub struct Job {
	pub input: TestInput,
	pub profile: Option<ProfilePin>,
	pub tool_references: std::collections::BTreeMap<String, EntityRef>,
	pub limits: TestLimits,
	pub context_window: usize,
	pub agent_max_steps: i32,
	pub model_provider: std::sync::Arc<dyn ModelProvider>,
	pub request: ModelRequest,
	pub initial_conversation: Vec<Value>,
	pub pinned_draft: Draft,
	/// What the session pinned from its Agent's plugin servers at admission.
	/// A pinned tool is answered by its explicit fixture and never dispatched.
	pub plugin_tools: PluginTools,
	/// `deferred@1`: the session's Exposure set and the request text around
	/// its per-turn Skill blocks and capability index. `None` under `legacy@1`.
	pub exposure: Option<SessionExposure>,
}

#[derive(Clone)]
pub struct SessionExposure {
	pub prefix: String,
	pub suffix: String,
	pub state: ExposureState,
}

pub async fn simulate(execution: &Execution, session_id: Uuid, job: &Job) -> Result<TestOutcome> {
	let Job {
		input,
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
	let mut exposure = job.exposure.clone();
	let mut calls = Vec::new();
	let mut input_tokens = 0_u64;
	let mut output_tokens = 0_u64;
	let mut usage_complete = true;
	let mut status = "blocked";
	let mut error = None;
	for step in 0..limits.max_steps.min(*agent_max_steps) {
		let admitted_session = execution.repository.read_session(session_id).await?;
		let still_running = admitted_session.status == "running";
		if !still_running {
			status = "stopped";
			break;
		}
		let mut authority = execution.repository.begin_execution().await?;
		authority.lock_identity().await?;
		let current_draft = authority.draft(job.pinned_draft.id, false).await?;
		authority
			.authorize_draft(&current_draft, "agent_draft.test", true)
			.await?;
		if current_draft.archived || current_draft.revision != job.pinned_draft.revision {
			return Err(Error::Conflict(
				"draft revision changed during session".into(),
			));
		}
		let entry = authority.validate_content(&job.pinned_draft).await?;
		let snapshot = authority
			.bindings(&job.pinned_draft, &entry)
			.await?
			.for_workbench_test();
		let saved = admitted_session.scenario["binding_snapshot"].clone();
		if saved != serde_json::to_value(&snapshot)? {
			return Err(Error::Conflict(
				"admitted test Binding graph changed".into(),
			));
		}
		if let Some(pin) = profile {
			let current = authority.profile(&pin.tenant, &pin.id).await?;
			if !current.enabled
				|| current.revision != pin.revision
				|| current.rules != serde_json::to_value(&pin.rules)?
			{
				return Err(Error::Conflict(
					"test profile changed during session".into(),
				));
			}
		}
		authority.commit().await?;
		let discoverable = match &exposure {
			Some(_) => deferred::Deferred::new(&snapshot, Some(&job.plugin_tools))?,
			None => None,
		};
		if exceeds_limits(
			&request,
			limits,
			*context_window,
			input_tokens,
			output_tokens,
			0,
		) {
			error = Some("test context exceeds configured input or model window limit".into());
			break;
		}
		let response = model_provider.infer(request.clone(), &NoProgress).await?;
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
		for mut call in response.tool_calls {
			if calls.len() >= limits.max_steps as usize {
				error = Some("test step limit reached".into());
				missing = true;
				break;
			}
			let real_rule = profile.as_ref().and_then(|pin| {
				tool_references
					.get(&call.name)
					.filter(|_| request.tools.iter().any(|tool| tool.name == call.name))
					.and_then(|selected| pin.rules.iter().find(|rule| selected == &rule.tool))
			});
			let fixture = input.fixtures.get(&call.name);
			let binding = snapshot.bindings.iter().find(|binding| {
				binding.alias.as_deref() == Some(&call.name) && binding.excluded_reason.is_none()
			});
			let admitted = match binding {
				Some(binding) => binding
					.narrow
					.apply(&mut call.arguments)
					.map_err(Error::from),
				None if pinned_plugin_tool(&job.plugin_tools, &call.name) => Ok(()),
				None => Err(Error::Forbidden),
			};
			let evaluated = match (&discoverable, exposure.as_mut(), &admitted) {
				(Some(discoverable), Some(session), Ok(())) => {
					// The next request resends the whole conversation, so a
					// result must leave it within the session's limits.
					let fits = |output: &Value, reserve: usize| {
						let mut next = request.clone();
						let mut retained = conversation.clone();
						retained.push(json!({"role":"tool","content":{"id":call.id,"name":call.name,"arguments":call.arguments,"result":output,"outcome":deferred::EVALUATED}}));
						next.context.legacy_mut().expect("Legacy request context")["conversation"] =
							json!(retained);
						!exceeds_limits(
							&next,
							limits,
							*context_window,
							input_tokens,
							output_tokens,
							reserve,
						)
					};
					discoverable.evaluate(binding, &call, &mut session.state, step, &fits)?
				}
				_ => None,
			};
			let approval = if admitted.is_ok() && evaluated.is_none() && real_rule.is_some() {
				approval_for(
					execution.repository.as_ref(),
					&job.plugin_tools,
					binding,
					&admitted_session.tenant,
					&call,
					&conversation,
				)
			} else {
				Ok(None)
			};
			let permitted = match &approval {
				Ok(Some(approval)) => {
					// The wait for a person can outlast the test's time limit or
					// meet a database error. Keep the model's tool-call message and
					// its usage durable first, so settling does not lose them.
					execution.repository.progress(session_id,json!(conversation),json!(calls),json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete})).await?;
					super::approval::ask(execution.repository.as_ref(), session_id, approval)
						.await?
				}
				Ok(None) => true,
				Err(_) => false,
			};
			let result = if let Err(reason) = admitted {
				missing = true;
				error = Some(reason.to_string());
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"denied","error":reason.to_string()})
			} else if let Some(deferred::Evaluated::NoRoom) = evaluated {
				// Nothing is retained: the next request could not carry it.
				missing = true;
				let reason = format!(
					"test context has no room for the {} result within the configured input or model window limit",
					call.name
				);
				error = Some(reason.clone());
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"denied","error":reason})
			} else if let Some(deferred::Evaluated::Result(output)) = evaluated {
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"result":output,"outcome":deferred::EVALUATED})
			} else if !permitted {
				missing = true;
				error = Some("test approval was refused".into());
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"denied","error":"test approval was refused"})
			} else if let Some(rule) = real_rule {
				match dispatch::invoke(
					&dispatch::Dispatch {
						repository: execution.repository.as_ref(),
						configuration: execution.configuration.as_ref(),
						transport: execution.transport.as_ref(),
						approval: approval.as_ref().ok().and_then(Option::as_ref),
					},
					session_id,
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
						let pending = execution
							.repository
							.read_session(session_id)
							.await?
							.tool_calls;
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
			execution.repository.progress(session_id,json!(conversation),json!(calls),json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete})).await?;
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
		request
			.context
			.legacy_mut()
			.expect("Legacy request context")["conversation"] = json!(conversation);
		// Load/Unload results of this response shape the next request.
		if let (Some(discoverable), Some(session)) = (&discoverable, exposure.as_mut()) {
			session.state.activate();
			let (text, tools) = discoverable.request(&snapshot, &session.state)?;
			request.instructions = format!("{}{text}{}", session.prefix, session.suffix);
			request.tools = tools;
		}
	}
	if status == "blocked" && error.is_none() {
		error = Some("test step limit reached".into());
	}
	Ok(TestOutcome {
		status: status.into(),
		conversation: json!(conversation),
		tool_calls: json!(calls),
		usage: json!({"input_tokens":input_tokens,"output_tokens":output_tokens,"usage_complete":usage_complete}),
		error,
	})
}

fn approval_for(
	repository: &dyn ExecutionRepository,
	plugins: &PluginTools,
	binding: Option<&aidash_domain::registry::bindings::ResolvedBinding>,
	tenant: &str,
	call: &aidash_domain::provider::ToolCall,
	conversation: &[Value],
) -> Result<Option<TestApproval>> {
	use aidash_domain::{
		policy::Resource,
		tool::{ToolConfig, providers::ToolDescriptor},
	};
	let approval =
		|kind: &str, id: String, attributes: Value, action, requires_approval| TestApproval {
			call: call.clone(),
			resource: Resource {
				tenant: tenant.into(),
				kind: kind.into(),
				id,
				attributes,
			},
			action,
			requires_approval,
		};
	if let Some(binding) = binding {
		let descriptor: ToolDescriptor = serde_json::from_value(binding.definition.config.clone())?;
		if (descriptor.provider == "core.egress@1" && descriptor.operation == "outbound_get")
			|| matches!(descriptor.transport.as_ref(), Some(ToolConfig::Native { operation, .. }) if operation == "http_get")
		{
			let url = call.arguments["url"].as_str().ok_or(Error::Forbidden)?;
			let parsed = url::Url::parse(url).map_err(|_| Error::Forbidden)?;
			let origin = parsed.origin().ascii_serialization();
			aidash_domain::capabilities::outbound::permitted_origin(
				url,
				std::slice::from_ref(&origin),
			)?;
			return Ok(Some(approval(
				"outbound",
				origin.clone(),
				json!({"origin":origin,"action":"network.get"}),
				"network.get",
				true,
			)));
		}
		if let Some((operation, _)) = aidash_domain::tool::providers::research_operation(
			&descriptor.provider,
			&descriptor.operation,
		) {
			use aidash_domain::research::Operation;
			if operation == Operation::Fetch
				&& ["url", "source_id", "document_id"]
					.iter()
					.filter(|key| call.arguments[**key].is_string())
					.count() != 1
			{
				return Err(Error::Forbidden);
			}
			let destination = match operation {
				Operation::Search => repository
					.search_destination(&descriptor.provider, &call.arguments)?
					.ok_or(Error::Forbidden)?,
				Operation::Fetch if call.arguments["document_id"].is_string() => return Ok(None),
				Operation::Fetch => {
					let source = call.arguments["source_id"].as_str();
					let address = call.arguments["url"]
						.as_str()
						.or_else(|| {
							conversation
								.iter()
								.rev()
								.filter_map(|item| {
									item["content"]["fixture"]["response"]["sources"].as_array()
								})
								.flatten()
								.find(|item| {
									item["source_id"].as_str() == source && source.is_some()
								})
								.and_then(|item| item["url"].as_str())
						})
						.ok_or(Error::Forbidden)?;
					aidash_domain::research::search::candidate_host(address)
						.ok_or(Error::Forbidden)?
				}
				Operation::Find | Operation::View => return Ok(None),
			};
			return Ok(Some(approval(
				"research",
				destination.clone(),
				json!({"destination":destination,"action":"research.disclose"}),
				"research.disclose",
				true,
			)));
		}
		if let Some(config @ (ToolConfig::Http { .. } | ToolConfig::Mcp { .. })) =
			descriptor.transport
		{
			return Ok(Some(approval(
				"tool",
				binding.identity.id.clone(),
				json!({"version":binding.identity.version,"operation":"invoke","action":"tool.call_unapproved"}),
				"tool.call_unapproved",
				!config.declares_read_only(),
			)));
		}
	}
	for server in &plugins.servers {
		if let Some(tool) = server.tools.iter().find(|tool| tool.alias == call.name) {
			let id = format!(
				"agent-plugin:{}/{}/{}",
				server.plugin, server.server, tool.name
			);
			return Ok(Some(approval(
				"tool",
				id,
				json!({"version":server.digest,"operation":"invoke","action":"tool.call_unapproved","plugin":server.plugin,"server":server.server,"plugin_tool":tool.name}),
				"tool.call_unapproved",
				!tool.read_only,
			)));
		}
	}
	Ok(None)
}

fn pinned_plugin_tool(pinned: &PluginTools, alias: &str) -> bool {
	pinned
		.servers
		.iter()
		.flat_map(|server| &server.tools)
		.any(|tool| tool.alias == alias)
}

/// Whether `request`, grown by `reserve` bytes, exceeds the session's input,
/// model window or total token limits.
fn exceeds_limits(
	request: &ModelRequest,
	limits: &TestLimits,
	context_window: usize,
	input_tokens: u64,
	output_tokens: u64,
	reserve: usize,
) -> bool {
	let estimated = request.estimated_total_tokens().saturating_add(reserve);
	request
		.input_body()
		.to_string()
		.len()
		.saturating_add(reserve)
		> limits.max_input_bytes as usize
		|| estimated > context_window
		|| input_tokens
			.saturating_add(output_tokens)
			.saturating_add(estimated as u64)
			> limits.max_total_tokens as u64
}

/// A runtime deadline is classified from durable evidence, never inferred as a safe retry.
pub enum Failure {
	Execution(Error),
	TimedOut,
}
pub async fn settle(
	repository: &dyn ExecutionRepository,
	session_id: Uuid,
	message: &str,
	result: std::result::Result<TestOutcome, Failure>,
) -> Result<()> {
	let outcome = match result {
		Ok(outcome) => outcome,
		Err(failure) => {
			let prior = repository.read_session(session_id).await?;
			let dispatched = has_unknown_call(&prior.tool_calls);
			// The test was waiting for its starter, not for the model.
			let awaiting = prior.scenario["pending_approval"]["state"] == "pending";
			let (status, error) = match failure {
				Failure::Execution(error) => (
					if dispatched {
						"outcome_unknown"
					} else {
						"failed"
					},
					error.to_string(),
				),
				Failure::TimedOut => (
					if dispatched {
						"outcome_unknown"
					} else {
						"timed_out"
					},
					if dispatched {
						"test timed out while an external call was in flight; its outcome is unknown"
					} else if awaiting {
						"test approval was not answered before the time limit"
					} else {
						"model request timed out; provider outcome is unknown"
					}
					.into(),
				),
			};
			TestOutcome {
				status: status.into(),
				conversation: prior
					.conversation
					.unwrap_or_else(|| json!([{"role":"user","content":message}])),
				tool_calls: prior.tool_calls.unwrap_or_else(|| json!([])),
				usage: prior.usage,
				error: Some(error),
			}
		}
	};
	repository.finish(session_id, &outcome).await
}
#[cfg(test)]
mod tests;
