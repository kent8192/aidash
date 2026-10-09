//! Sandbox reasoning uses explicit fixtures or confined dispatch, never a runtime Tool executor.
use super::dispatch;
use crate::{
	Error, Result,
	ports::{
		Credentials, ModelProvider, NoProgress,
		registry::workbench::{
			profile::ProfileConfiguration,
			sandbox::{dispatch::RealToolTransport, execution::ExecutionRepository},
		},
	},
};
use aidash_domain::{
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
	pub credentials: Arc<dyn Credentials>,
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
	pub model_credential: Option<(String, Vec<u8>)>,
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
	let mut calls = Vec::new();
	let mut input_tokens = 0_u64;
	let mut output_tokens = 0_u64;
	let mut usage_complete = true;
	let mut status = "blocked";
	let mut error = None;
	for _ in 0..limits.max_steps.min(*agent_max_steps) {
		let admitted_session = execution.repository.read_session(session_id).await?;
		let still_running = admitted_session.status == "running";
		if !still_running {
			status = "stopped";
			break;
		}
		if let Some((name, fingerprint)) = &job.model_credential
			&& dispatch::credential_fingerprint(execution.credentials.as_ref(), name)?
				!= *fingerprint
		{
			return Err(Error::Conflict(
				"model credential changed during test".into(),
			));
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
		let snapshot = authority.bindings(&job.pinned_draft, &entry).await?;
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
			let admitted = snapshot
				.bindings
				.iter()
				.find(|binding| {
					binding.alias.as_deref() == Some(&call.name)
						&& binding.excluded_reason.is_none()
				})
				.ok_or(Error::Forbidden)
				.and_then(|binding| {
					binding
						.narrow
						.apply(&mut call.arguments)
						.map_err(Error::from)
				});
			let result = if let Err(reason) = admitted {
				missing = true;
				error = Some(reason.to_string());
				json!({"id":call.id,"name":call.name,"arguments":call.arguments,"outcome":"denied","error":reason.to_string()})
			} else if let Some(rule) = real_rule {
				match dispatch::invoke(
					&dispatch::Dispatch {
						repository: execution.repository.as_ref(),
						credentials: execution.credentials.as_ref(),
						configuration: execution.configuration.as_ref(),
						transport: execution.transport.as_ref(),
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
		request.context["conversation"] = json!(conversation);
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
