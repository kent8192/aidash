//! Duty Router and Watcher scripts (#1011, ADR 0142). A Run that executes a
//! Duty's script makes its own tool calls instead of asking a model: its Ready
//! step enters ToolCall with the calls [`scripts::plan`] makes, each runs
//! through the bound `shell` or `code_interpreter` tool under the Run's own
//! authority and waits for its Pod operation like any call, and the script's
//! stdout completes the Task. A script Run never records a Model Turn,
//! publishes text, recalls memory or reaches Thinking.
use super::*;
use aidash_domain::duties::scripts::{self, Invocation, Language};
use std::sync::Arc;

/// Whether a pending response is a script Run's own: every call carries the
/// script prefix. Only then does a step ask the store for the script.
pub(super) fn script_response(response: &aidash_domain::provider::ModelResponse) -> bool {
	!response.tool_calls.is_empty()
		&& response
			.tool_calls
			.iter()
			.all(|call| call.id.starts_with(scripts::CALL_PREFIX))
}

/// Why a script cannot run on this Run's Agent.
fn unavailable(language: Language) -> String {
	format!(
		"this Duty's Agent cannot run {} scripts: approve {} for the Duty's Node in the Catalog and bind it to the Agent",
		language.as_str(),
		language.operation()
	)
}

/// The bound tool that runs `operation`, found by its operation identity
/// rather than its alias, as an Operation wait finds its poll tool.
fn bound(run: &Run, tools: &Tools, operation: &str) -> Option<(String, Arc<dyn ExecutionTool>)> {
	tools
		.iter()
		.find(|(_, tool)| match &tool.contract().identity {
			ToolIdentity::Builtin(name) => name == &format!("builtin:{operation}"),
			ToolIdentity::Descriptor(identity) => run
				.context
				.binding_snapshot
				.as_ref()
				.is_some_and(|snapshot| {
					snapshot.bindings.iter().any(|binding| {
						binding.identity == *identity
							&& binding.definition.config["operation"] == operation
							&& binding.definition.config["provider"]
								.as_str()
								.is_some_and(|provider| provider.starts_with("core."))
					})
				}),
			_ => false,
		})
		.map(|(name, tool)| (name.clone(), tool.clone()))
}

/// The fields of an operation result, inside its tool Envelope when it has one.
fn body(output: &Value) -> &Value {
	output
		.get("result")
		.filter(|result| result.is_object())
		.unwrap_or(output)
}

/// Whether the operation behind a call result is still under way.
fn active(output: &Value) -> bool {
	matches!(
		output["status"].as_str(),
		Some("prepared" | "submitted" | "running" | "cancelling")
	)
}

/// What went wrong with a settled call, if anything: a refused, failed,
/// cancelled, timed-out or uncertain operation, or (for a write) a nonzero exit.
fn problem(output: &Value, last: bool) -> Option<String> {
	let fields = body(output);
	let error = match fields.get("error").or_else(|| output.get("error")) {
		Some(Value::String(message)) => Some(message.clone()),
		Some(Value::Object(error)) => Some(format!(
			"{}: {}",
			error.get("code").and_then(Value::as_str).unwrap_or("ERROR"),
			error
				.get("message")
				.and_then(Value::as_str)
				.unwrap_or_default()
		)),
		_ => None,
	};
	let status = output["status"].as_str().unwrap_or("completed");
	if status != "completed" || error.is_some() {
		let tail: String = {
			let text = fields["output"].as_str().unwrap_or_default();
			let start = text
				.char_indices()
				.rev()
				.nth(499)
				.map_or(0, |(index, _)| index);
			text[start..].trim().to_owned()
		};
		let mut message = format!("the script's {status} call ended");
		if let Some(error) = error {
			message.push_str(&format!(" with {error}"));
		}
		if !tail.is_empty() {
			message.push_str(&format!(": {tail}"));
		}
		return Some(message);
	}
	// The last call's exit code is the script's, which its output explains.
	match fields["exit_code"].as_i64() {
		Some(code) if code != 0 && !last => Some(format!(
			"the script could not be written into the Scratch Space (exit code {code}): {}",
			fields["output"].as_str().unwrap_or_default().trim()
		)),
		_ => None,
	}
}

impl Executor<'_> {
	/// Ready → the script's calls, without a model. A Run whose Agent has no
	/// bound tool for the script's language fails with the reason.
	pub(super) async fn start_duty_script(
		&self,
		run: &mut Run,
		token: Uuid,
		invocation: &Invocation,
	) -> Result<()> {
		let tools = self.environment.binding_resolver().tools(run).await?;
		let Some((name, _)) = bound(run, &tools, invocation.language.operation()) else {
			return Err(Error::Invalid(unavailable(invocation.language)));
		};
		let directory = scripts::directory(run.task_id);
		let tool_calls = scripts::plan(invocation, &directory)
			.into_iter()
			.enumerate()
			.map(|(index, command)| aidash_domain::provider::ToolCall {
				id: format!("{}{index}", scripts::CALL_PREFIX),
				name: name.clone(),
				arguments: json!({
					"idempotency_key": scripts::call_key(run.id, index),
					"command": command,
					"expected_revision": 0,
				}),
			})
			.collect();
		run.state = RunState::ToolCall(Box::new(ToolCallState {
			response: aidash_domain::provider::ModelResponse {
				tool_calls,
				usage_complete: true,
				..Default::default()
			},
			response_epoch: response_epoch(run.revision, run.step),
			..Default::default()
		}));
		self.environment
			.store()
			.save_run(run, token, "execution.started")
			.await
	}

	/// One step of a script Run in ToolCall: the call at the cursor, and after
	/// the last one the Task's end.
	pub(super) async fn duty_script_step(
		&self,
		run: &mut Run,
		token: Uuid,
		home: &dyn ExecutionHome,
		guard: Option<&dyn ExecutionAuthority>,
		invocation: &Invocation,
	) -> Result<()> {
		let store = self.environment.store();
		let state = run.state.tool()?.clone();
		let cursor = state.cursor;
		let Some(call) = state.response.tool_calls.get(cursor).cloned() else {
			return Err(Error::Invalid(
				"a Duty script Run has no call left to make".into(),
			));
		};
		let last = cursor + 1 == state.response.tool_calls.len();
		let tools = self.environment.binding_resolver().tools(run).await?;
		let Some(tool) = tools.get(&call.name).cloned() else {
			return Err(Error::Invalid(unavailable(invocation.language)));
		};
		let contract = tool.contract();
		// The script runs under the Run's current authority, as a model's call would.
		if let Some(guard) = guard {
			guard.tool(&call, &contract).await?;
		}
		let key = format!("{}:{}:{}", run.id, state.response_epoch, cursor);
		let key = match state.approval {
			Some(approval) => format!("{key}:approval:{approval}"),
			None => key,
		};
		let journaled = store
			.invocation_start(
				run,
				token,
				&key,
				&call.name,
				&call.arguments,
				contract.replay_safe(),
			)
			.await?;
		if journaled.status == "UNCERTAIN" {
			return Err(Error::Invalid(
				"the script may have run before the worker stopped, so it is not run again".into(),
			));
		}
		let waited = run.state.tool_mut()?.wait_result.take();
		let resumed = waited.is_some();
		let output = match waited {
			Some(output) => output,
			None if journaled.status == "COMPLETED" => journaled
				.result
				.ok_or_else(|| Error::Conflict("completed invocation has no result".into()))?,
			None => tool.invoke(run, call.arguments.clone(), &key).await?,
		};
		let framework = if resumed {
			FrameworkResult::Ordinary
		} else {
			FrameworkResult::decode(&contract, &output, &call.arguments)?
		};
		match framework {
			FrameworkResult::Operation { id, seconds } => {
				return self.wait_for_operation(run, token, id, seconds).await;
			}
			// An approval raised inside the call waits for its person; the call
			// then runs again under the approval's own key.
			FrameworkResult::Approval(approval_id) => {
				let mut resume = run.state.tool()?.clone();
				resume.approval = Some(approval_id);
				run.state = RunState::Waiting(Box::new(WaitingState::Approval {
					approval: ApprovalRef::Capability { approval_id },
					resume: Box::new(resume),
				}));
				return store
					.save_run(run, token, "execution.waiting_for_approval")
					.await;
			}
			FrameworkResult::Human(_) | FrameworkResult::Wait(_) => {
				return Err(Error::Invalid(
					"a Duty script call cannot wait for a person or for Tasks".into(),
				));
			}
			FrameworkResult::Ordinary => {}
		}
		// Woken at its ceiling while the command still runs: wait again.
		if resumed && active(&output) {
			let id = serde_json::from_value(output["operation_id"].clone())
				.map_err(|_| Error::Invalid("operation result is missing a valid ID".into()))?;
			return self.wait_for_operation(run, token, id, 300).await;
		}
		let output = if journaled.status == "COMPLETED" {
			output
		} else {
			store.invocation_finish(run, token, &key, &output).await?
		};
		if let Some(problem) = problem(&output, last) {
			return Err(Error::Invalid(problem));
		}
		if !last {
			run.state.tool_mut()?.advance(cursor + 1);
			return store.save_run(run, token, "execution.tool_recorded").await;
		}
		let text = self
			.full_output(run, guard, &tools, invocation.language, &output)
			.await?;
		let stdout = scripts::script_output(&text).map_err(Error::Invalid)?;
		let task = home.task().await?;
		let artifact = ArtifactInput {
			kind: "text".into(),
			name: result_artifact_name(&task.title),
			content: json!(stdout),
		};
		if let Some(guard) = guard {
			guard
				.action("artifact.create", "artifact", run.task_id)
				.await?;
			guard.action("task.complete", "task", run.task_id).await?;
		}
		home.complete(&format!("{}:complete", run.id), &artifact, &[])
			.await?;
		run.state = RunState::Completed(TerminalState {});
		store.save_run(run, token, "execution.completed").await
	}

	async fn wait_for_operation(
		&self,
		run: &mut Run,
		token: Uuid,
		operation_id: Uuid,
		seconds: i64,
	) -> Result<()> {
		// The operation's end wakes the Run; the wake time is only a ceiling.
		run.state = RunState::Waiting(Box::new(WaitingState::Operation {
			operation_id,
			wake_at: chrono::Utc::now() + chrono::Duration::seconds(seconds),
			resume: Box::new(run.state.tool()?.clone()),
		}));
		self.environment
			.store()
			.save_run(run, token, "execution.waiting_for_operation")
			.await
	}

	/// Every page of the last call's output: a tool result shows one page, so
	/// the rest is read through the language's poll operation. Output the
	/// Runner cut short is refused rather than read as complete.
	async fn full_output(
		&self,
		run: &Run,
		guard: Option<&dyn ExecutionAuthority>,
		tools: &Tools,
		language: Language,
		output: &Value,
	) -> Result<String> {
		let fields = body(output);
		let mut text = fields["output"].as_str().unwrap_or_default().to_owned();
		let mut truncated = fields["truncated"] == true;
		let mut next = fields["next_offset"].as_u64();
		let operation = output["operation_id"].clone();
		let poll = bound(run, tools, language.poll_operation());
		while let Some(offset) = next {
			if text.len() > scripts::MAX_OUTPUT_BYTES {
				return Err(Error::Invalid(format!(
					"the script printed more than {} bytes",
					scripts::MAX_OUTPUT_BYTES
				)));
			}
			let arguments = json!({"operation_id": operation, "offset": offset, "wait_seconds": 0});
			let page = match &poll {
				Some((name, tool)) => {
					let call = aidash_domain::provider::ToolCall {
						id: format!("{}poll", scripts::CALL_PREFIX),
						name: name.clone(),
						arguments: arguments.clone(),
					};
					if let Some(guard) = guard {
						guard.tool(&call, &tool.contract()).await?;
					}
					tool.invoke(run, arguments, "").await?
				}
				None => {
					self.environment
						.poll_operation(run, language.poll_operation(), arguments)
						.await?
				}
			};
			let fields = body(&page);
			let chunk = fields["output"].as_str().unwrap_or_default();
			truncated |= fields["truncated"] == true;
			text.push_str(chunk);
			next = fields["next_offset"].as_u64();
			// A page that moves nothing would read forever.
			if chunk.is_empty() && next == Some(offset) {
				break;
			}
		}
		if truncated {
			return Err(Error::Invalid(
				"the script's output was cut at the Runner's limit, so it cannot be read whole"
					.into(),
			));
		}
		Ok(text)
	}
}
