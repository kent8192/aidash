//! Aidash, rather than the SDK's automatic MRTR loop, owns human waits and replay.
use super::*;
use aidash_application::capabilities::approvals::Refusal;
use aidash_application::ports::{
	capabilities::approvals::SamplingApprovalRequest, tools::PluginInputs,
};
use aidash_domain::agent_plugin::interaction::{
	CallCheckpoint, ElicitationRequest, InputRequest, RequiredInput, SamplingRequest,
	SamplingTarget, SentRound, ToolRound,
};
use aidash_domain::provider::{ModelContext, ModelRequest, SamplingContext};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(super) struct Call<'a> {
	pub tool: &'a dyn ExecutionTool,
	pub call: &'a aidash_domain::provider::ToolCall,
	pub key: &'a str,
	pub agent: &'a ExecutionAgent,
	pub guard: &'a dyn ExecutionAuthority,
	pub visibility: &'a mut dyn ExecutionVisibility,
}

struct LiveInput {
	id: String,
	request: InputRequest,
	reply: oneshot::Sender<Result<Value>>,
}
struct LiveInputs(mpsc::Sender<LiveInput>);
#[async_trait]
impl PluginInputs for LiveInputs {
	async fn request(&self, id: String, request: InputRequest) -> Result<Value> {
		let (reply, response) = oneshot::channel();
		self.0
			.try_send(LiveInput { id, request, reply })
			.map_err(|_| Error::Invalid("MCP_INPUT_OVERLAP".into()))?;
		response
			.await
			.map_err(|_| Error::Invalid("MCP_INPUT_CLOSED".into()))?
	}
}
enum Input {
	Ready(Value),
	Wait(WaitingState),
}

impl Executor<'_> {
	pub(super) async fn mcp_call(
		&self,
		run: &mut Run,
		token: Uuid,
		mut call: Call<'_>,
	) -> Result<Option<Value>> {
		let store = self.environment.store();
		let mut checkpoint = run
			.state
			.tool()?
			.mcp
			.clone()
			.unwrap_or_else(|| CallCheckpoint::new(call.key.into()));
		if checkpoint.key != call.key {
			return Err(Error::Conflict(
				"MCP checkpoint belongs to another call".into(),
			));
		}
		if run.state.tool()?.mcp.is_none() {
			run.state.tool_mut()?.mcp = Some(checkpoint.clone());
			store.checkpoint_mcp(run, token).await?;
		}
		loop {
			// A callback can have replaced the input checkpoint while a send
			// remained open. Recovery must reconcile that send before touching
			// the callback or advancing the modern continuation counter.
			let recovering_send = checkpoint.sent.is_some();
			if recovering_send {
				if let Some(active) = checkpoint.active.take() {
					checkpoint.waited_millis = checkpoint.waited_millis.saturating_add(
						(chrono::Utc::now().min(active.deadline) - active.started_at)
							.num_milliseconds()
							.max(0) as u64,
					);
				}
			} else {
				while let Some((id, request)) = checkpoint
					.next_input()
					.map(|(id, request)| (id.to_owned(), request.clone()))
				{
					match self
						.mcp_input(run, token, &mut call, &mut checkpoint, &id, &request)
						.await
					{
						Ok(Input::Ready(value)) => {
							checkpoint.answer(&id, value, chrono::Utc::now())?
						}
						Ok(Input::Wait(wait)) => {
							run.state.tool_mut()?.mcp = Some(checkpoint.clone());
							// Rebuild the resume after the input established its deadline.
							let resume = Box::new(run.state.tool()?.clone());
							run.state = RunState::Waiting(Box::new(match wait {
								WaitingState::Approval { approval, .. } => {
									WaitingState::Approval { approval, resume }
								}
								WaitingState::Human {
									request_id,
									answer_by,
									..
								} => WaitingState::Human {
									request_id,
									answer_by,
									resume: ResumeState::ToolCall(resume),
								},
								_ => return Err(Error::Invalid("invalid MCP wait".into())),
							}));
							store.save_run(run, token, "run.mcp_waiting").await?;
							return Ok(None);
						}
						Err(error) => {
							return self
								.mcp_error(run, token, &checkpoint, error)
								.await
								.map(Some);
						}
					}
				}
				// A checkpoint's continuation number is advanced only after every
				// input has a result. The journal, not network state, owns send replay.
				if checkpoint.input.is_some()
					&& !checkpoint.wire_pending
					&& let Err(error) = checkpoint.continue_call(self.environment.mcp_limits())
				{
					return self
						.mcp_error(run, token, &checkpoint, error.into())
						.await
						.map(Some);
				}
				checkpoint.wire_pending = true;
				let wire_input = json!({"call":call.call,"request_state":checkpoint.input.as_ref().and_then(|input| input.request_state.as_ref()),"input_responses":checkpoint.responses});
				checkpoint.sent = Some(SentRound {
					key: format!("{}:mcp:round:{}", call.key, checkpoint.continuation_sends),
					input: wire_input,
				});
			}
			let sent = checkpoint.sent.as_ref().expect("MCP send identity");
			let round_key = sent.key.clone();
			if round_key != format!("{}:mcp:round:{}", call.key, checkpoint.continuation_sends)
				|| sent.input["call"] != serde_json::to_value(call.call)?
			{
				return Err(Error::Conflict(
					"MCP sent round belongs to another call".into(),
				));
			}
			let wire_input = sent.input.clone();
			// Admission persists the Run and journal row atomically. Install the
			// snapshot before admission so even a loss before the first callback
			// retains the exact request that was sent.
			run.state.tool_mut()?.mcp = Some(checkpoint.clone());
			let invocation = store
				.invocation_start(
					run,
					token,
					&round_key,
					"mcp.round",
					&wire_input,
					call.tool.replay_safe(),
				)
				.await?;
			if invocation.status == "UNCERTAIN"
				|| (recovering_send
					&& invocation.status != "COMPLETED"
					&& !invocation.retrying
					&& !call.tool.replay_safe())
			{
				let reason = if invocation.uncertain_reason.as_deref() == Some("answer_lost") {
					"its answer was lost after the request was sent"
				} else {
					"the worker stopped"
				};
				let prompt = format!(
					"MCP tool {} may have completed before {reason}. Reconcile its effect and answer with a JSON object containing result. It will not be sent again. Invocation: {round_key}",
					call.call.name
				);
				run.state.tool_mut()?.mcp = Some(checkpoint);
				store
					.reconciliation_request(run, token, &round_key, &prompt)
					.await?;
				store.save_run(run, token, "run.waiting").await?;
				return Ok(None);
			}
			let round = if invocation.status == "COMPLETED" {
				let raw = invocation
					.result
					.ok_or_else(|| Error::Conflict("MCP round has no saved result".into()))?;
				serde_json::from_value::<ToolRound>(raw)?
			} else {
				let (sender, mut inputs) = mpsc::channel(1);
				let state = run.clone();
				let input_handler: Arc<dyn PluginInputs> = Arc::new(LiveInputs(sender));
				let tool = call.tool;
				// Freeze the payload of this send while live callbacks update
				// the logical call's cumulative wait in the outer checkpoint.
				let wire_checkpoint = checkpoint.clone();
				let future = tool.mcp_round(
					&state,
					call.call.arguments.clone(),
					&round_key,
					Some(&wire_checkpoint),
					Some(input_handler),
				);
				tokio::pin!(future);
				let mut wire_remaining = std::time::Duration::from_secs(120);
				let result = loop {
					let wire_started = tokio::time::Instant::now();
					tokio::select! {
						result = &mut future => break result,
						_ = tokio::time::sleep(wire_remaining) => break Err(Error::External("MCP request timed out".into())),
						Some(input) = inputs.recv() => {
							wire_remaining = wire_remaining.saturating_sub(wire_started.elapsed());
							let answer = self.mcp_live_input(run, token, &mut call, &checkpoint, input.id, input.request).await;
							if answer.as_ref().is_err_and(|error| !matches!(error, Error::Invalid(code) | Error::Domain(aidash_domain::Error::Invalid(code)) if code.starts_with("MCP_"))) {
								let error = answer.expect_err("failed input");
								let _ = input.reply.send(Err(Error::Invalid("MCP_INPUT_CLOSED".into())));
								self.environment.close_plugin_sessions(run.id).await;
								return Err(error);
							}
							// A live callback checkpoints its own human wait while
							// this wire round still owns the logical continuation.
							if let Some(saved) = &run.state.tool()?.mcp {
								checkpoint.waited_millis = checkpoint.waited_millis.max(saved.waited_millis);
							}
							let _ = input.reply.send(answer);
						}
					}
				};
				let result = match result {
					Ok(result) => result,
					Err(
						Error::Invalid(message)
						| Error::Domain(aidash_domain::Error::Invalid(message)),
					) => ToolRound::Complete(json!({"error": message})),
					Err(Error::ToolNotDelivered {
						message,
						retry_after,
					}) => {
						let failure = ToolRound::Complete(
							json!({"error":{"kind":"not_delivered","message":message}}),
						);
						let count = store
							.invocation_not_delivered(
								run,
								token,
								&round_key,
								&serde_json::to_value(&failure)?,
							)
							.await?;
						if count > aidash_domain::semantic::OUTAGE_RETRIES {
							failure
						} else {
							return Err(Error::ToolRetry {
								message,
								count,
								retry_after,
							});
						}
					}
					Err(error @ Error::External(_)) => {
						store.invocation_answer_lost(run, token, &round_key).await?;
						return Err(error);
					}
					// Leave the sent request unfinished. Recovery reconciles it.
					Err(error) => return Err(error),
				};
				let saved = store
					.invocation_finish(run, token, &round_key, &serde_json::to_value(&result)?)
					.await?;
				serde_json::from_value(saved)?
			};
			match round {
				// The immutable journal result and the caller's atomic tool event /
				// cursor save consume this logical result once across worker loss.
				ToolRound::Complete(value) => return Ok(Some(value)),
				ToolRound::InputRequired(input) => {
					checkpoint.sent = None;
					if let Err(error) = checkpoint.accept(input) {
						return self
							.mcp_error(run, token, &checkpoint, error.into())
							.await
							.map(Some);
					}
					run.state.tool_mut()?.mcp = Some(checkpoint.clone());
					store.checkpoint_mcp(run, token).await?;
				}
			}
		}
	}

	async fn mcp_error(
		&self,
		run: &Run,
		_token: Uuid,
		checkpoint: &CallCheckpoint,
		error: Error,
	) -> Result<Value> {
		let code = match error {
			Error::Invalid(code) | Error::Domain(aidash_domain::Error::Invalid(code))
				if code.starts_with("MCP_") =>
			{
				code
			}
			other => return Err(other),
		};
		self.environment
			.store()
			.emit(
				Some(run.workspace_id),
				"mcp.input_failed",
				json!({"run_id":run.id,"call":checkpoint.key,"code":code}),
			)
			.await?;
		Ok(json!({"error":code}))
	}

	async fn mcp_live_input(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &mut Call<'_>,
		logical: &CallCheckpoint,
		id: String,
		request: InputRequest,
	) -> Result<Value> {
		// The outer loop may have answered modern inputs since the last saved
		// checkpoint. Its logical state owns this callback's remaining budget.
		let mut checkpoint = logical.clone();
		checkpoint.accept(RequiredInput {
			request_state: None,
			requests: [(id.clone(), request.clone())].into(),
		})?;
		checkpoint.wire_pending = true;
		let mut saved_wait = false;
		loop {
			let outcome = self
				.mcp_input(run, token, call, &mut checkpoint, &id, &request)
				.await;
			let outcome = match outcome {
				Ok(outcome) => outcome,
				Err(error) => {
					if let Some(active) = checkpoint.active.take() {
						checkpoint.waited_millis = checkpoint.waited_millis.saturating_add(
							(chrono::Utc::now().min(active.deadline) - active.started_at)
								.num_milliseconds()
								.max(0) as u64,
						);
					}
					checkpoint.input = logical.input.clone();
					checkpoint.responses = logical.responses.clone();
					checkpoint.wire_pending = true;
					run.state.tool_mut()?.mcp = Some(checkpoint.clone());
					self.environment.store().checkpoint_mcp(run, token).await?;
					self.mcp_error(
						run,
						token,
						&checkpoint,
						Error::Invalid("MCP_INPUT_DENIED".into()),
					)
					.await?;
					return Err(error);
				}
			};
			match outcome {
				Input::Ready(answer) => {
					checkpoint.answer(&id, answer.clone(), chrono::Utc::now())?;
					// Keep cumulative waiting; this is not a modern continuation.
					checkpoint.input = logical.input.clone();
					checkpoint.wire_pending = true;
					checkpoint.responses = logical.responses.clone();
					run.state.tool_mut()?.mcp = Some(checkpoint);
					self.environment.store().checkpoint_mcp(run, token).await?;
					return Ok(answer);
				}
				Input::Wait(wait) => {
					run.state.tool_mut()?.mcp = Some(checkpoint.clone());
					let executing = run.state.tool()?.clone();
					let mut waiting = run.clone();
					waiting.state = RunState::Waiting(Box::new(match wait {
						WaitingState::Approval { approval, .. } => WaitingState::Approval {
							approval,
							resume: Box::new(executing.clone()),
						},
						WaitingState::Human {
							request_id,
							answer_by,
							..
						} => WaitingState::Human {
							request_id,
							answer_by,
							resume: ResumeState::ToolCall(Box::new(executing.clone())),
						},
						_ => return Err(Error::Invalid("invalid MCP wait".into())),
					}));
					// Save WAITING while retaining this lease and the live MCP call.
					if !saved_wait {
						self.environment
							.store()
							.checkpoint_mcp(&mut waiting, token)
							.await?;
						run.revision = waiting.revision;
						saved_wait = true;
					}
					call.guard.suspend().await?;
					call.visibility.suspend().await?;
					tokio::time::sleep(std::time::Duration::from_millis(500)).await;
					call.visibility.resume().await?;
					call.guard.resume().await?;
					call.guard.inference().await?;
				}
			}
		}
	}

	async fn mcp_input(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &mut Call<'_>,
		checkpoint: &mut CallCheckpoint,
		id: &str,
		request: &InputRequest,
	) -> Result<Input> {
		let limits = self.environment.mcp_limits();
		let input_key = format!(
			"{}:mcp:input:{}:{}",
			call.key,
			checkpoint.continuation_sends,
			aidash_domain::registry::rules::digest(&json!(id))
		);
		let now = chrono::Utc::now();
		match request.method.as_str() {
			"sampling/createMessage" => {
				let model_entry = self
					.environment
					.catalog()
					.get_for_run(run, &call.agent.model.id, &call.agent.model.version)
					.await?;
				let model: ModelConfig = serde_json::from_value(model_entry.config)?;
				let sample = SamplingRequest::parse(
					&request.params,
					limits.sampling_tokens.min(model.output_token_limit()),
				)?;
				if let Some(saved) = self
					.environment
					.store()
					.completed_invocation(
						run,
						token,
						&input_key,
						"mcp.sampling",
						&serde_json::to_value(&sample)?,
					)
					.await?
				{
					return sampled(saved).map(Input::Ready);
				}
				let new_wait = checkpoint.active.is_none();
				let deadline = checkpoint.begin_wait(id, now, limits.human_wait_seconds, limits)?;
				if new_wait {
					run.state.tool_mut()?.mcp = Some(checkpoint.clone());
					self.environment.store().checkpoint_mcp(run, token).await?;
				}
				let ToolIdentity::AgentPlugin(tool) = call.tool.contract().identity else {
					return Err(Error::Invalid("MCP_SAMPLING_TARGET_INVALID".into()));
				};
				let target = SamplingTarget {
					agent: run.agent_id.clone(),
					plugin: tool.plugin,
					server: tool.server,
					digest: tool.digest,
					origin: call
						.tool
						.external_target()
						.and_then(|url| url::Url::parse(&url).ok())
						.map(|url| url.origin().ascii_serialization())
						.unwrap_or_else(|| "stdio".into()),
				};
				match call
					.guard
					.sampling_approval(
						run,
						SamplingApprovalRequest {
							key: input_key.clone(),
							target,
							request: sample.clone(),
							expires_at: deadline,
							area: None,
						},
					)
					.await?
				{
					ToolCallApproval::Approved { .. } => {
						if let Some(active) = &checkpoint.active {
							checkpoint.waited_millis = checkpoint.waited_millis.saturating_add(
								(now.min(active.deadline) - active.started_at)
									.num_milliseconds()
									.max(0) as u64,
							);
						}
						checkpoint.active = None;
						// Sampling admission persists the Run before provider I/O;
						// the human wait has already ended at this point.
						run.state.tool_mut()?.mcp = Some(checkpoint.clone());
						self.mcp_sample(run, token, call, &input_key, sample, model)
							.await
							.map(Input::Ready)
					}
					ToolCallApproval::Refused(Refusal::Expired) => {
						Err(Error::Invalid("MCP_SAMPLING_EXPIRED".into()))
					}
					ToolCallApproval::Refused(_) => {
						Err(Error::Invalid("MCP_SAMPLING_REFUSED".into()))
					}
					ToolCallApproval::Pending { approval_id, .. } if deadline > now => {
						Ok(Input::Wait(WaitingState::Approval {
							approval: ApprovalRef::Capability { approval_id },
							resume: Box::new(run.state.tool()?.clone()),
						}))
					}
					_ => Err(Error::Invalid("MCP_SAMPLING_EXPIRED".into())),
				}
			}
			"elicitation/create" => {
				let form = ElicitationRequest::parse(&request.params)?;
				if call.guard.approval_mode(run).await? == Some(ApprovalMode::DontAsk) {
					return Ok(Input::Ready(json!({"action":"decline"})));
				}
				let seconds = call
					.guard
					.answer_deadline(run)
					.await?
					.map_or(limits.human_wait_seconds, |seconds| seconds.max(0) as u64);
				let new_wait = checkpoint.active.is_none();
				let deadline = checkpoint.begin_wait(id, now, seconds, limits)?;
				if new_wait {
					run.state.tool_mut()?.mcp = Some(checkpoint.clone());
					self.environment.store().checkpoint_mcp(run, token).await?;
				}
				let prompt =
					json!({"message": form.message, "schema": form.schema, "answer_by":deadline})
						.to_string();
				let home = self.environment.home(run);
				let question = home
					.human_request(run, "MCP_ELICITATION", &prompt, &input_key)
					.await?;
				let answer = if let Some(answer) = question.response {
					Some(answer)
				} else if deadline <= now {
					match home
						.close_human_request(question.id, json!({"action":"decline"}))
						.await
					{
						Ok(question) => question.response,
						Err(Error::Conflict(_)) => {
							home.human_request_by_id(question.id).await?.response
						}
						Err(error) => return Err(error),
					}
				} else {
					None
				};
				match answer {
					Some(answer) => {
						let answer = form.response(&answer)?;
						if answer["action"] == "accept" {
							aidash_application::tools::validate_form_answer(
								&form.schema,
								&answer["content"],
							)?;
						}
						Ok(Input::Ready(answer))
					}
					None => Ok(Input::Wait(WaitingState::Human {
						request_id: question.id,
						answer_by: Some(deadline),
						resume: ResumeState::ToolCall(Box::new(run.state.tool()?.clone())),
					})),
				}
			}
			_ => Err(Error::Invalid("MCP_INPUT_UNSUPPORTED".into())),
		}
	}

	async fn mcp_sample(
		&self,
		run: &Run,
		token: Uuid,
		call: &mut Call<'_>,
		key: &str,
		sample: SamplingRequest,
		model: ModelConfig,
	) -> Result<Value> {
		let store = self.environment.store();
		let input = serde_json::to_value(&sample)?;
		let journal = store
			.invocation_start(run, token, key, "mcp.sampling", &input, false)
			.await?;
		if journal.status == "UNCERTAIN" {
			return Err(Error::Invalid("MCP_SAMPLING_OUTCOME_UNKNOWN".into()));
		}
		if journal.status == "COMPLETED" {
			let value = journal
				.result
				.ok_or_else(|| Error::Conflict("sampling result is missing".into()))?;
			return sampled(value);
		}
		call.guard.inference().await?;
		let request = ModelRequest {
			instructions: sample.system_prompt,
			context: ModelContext::Sampling(SamplingContext {
				sampling_messages: sample.messages,
			}),
			max_output_tokens: sample.max_tokens,
			disable_provider_transforms: true,
			..Default::default()
		};
		request
			.ensure_fits(model.context_window)
			.map_err(|_| Error::Invalid("MCP_SAMPLING_REQUEST_LIMIT".into()))?;
		let provider = self.environment.provider(model.clone())?;
		let attempt = InferenceAttemptId::new();
		let input_reservation = request
			.estimated_total_tokens()
			.saturating_sub(sample.max_tokens as usize);
		let reservation = match call
			.guard
			.reserve_inference(attempt.0, input_reservation, sample.max_tokens, &request)
			.await
		{
			Ok(reservation) => reservation,
			Err(Error::Invalid(message))
				if message == aidash_domain::generation::inference::TOKEN_BUDGET_EXHAUSTED =>
			{
				store
					.invocation_finish(run, token, key, &json!({"mcp_error":"budget"}))
					.await?;
				return Err(Error::Invalid("MCP_SAMPLING_BUDGET".into()));
			}
			Err(error) => return Err(error),
		};
		let dispatched = store
			.record_usage_dispatch(
				run,
				token,
				&UsageDispatch {
					attempt: attempt.0,
					lease: token,
					response_epoch: run.state.tool()?.response_epoch,
					model_id: call.agent.model.id.clone(),
					model_version: call.agent.model.version.clone(),
					projection_version: aidash_domain::projection::ProjectionVersion::Legacy
						.number(),
					upstream_model: model.model_id.clone(),
					purpose: UsagePurpose::Main,
					requested_route: model.provider_route.as_ref().map(|route| route.tag.clone()),
					// Sampling does not render the Run's context projection.
					shadow_estimates: Vec::new(),
					estimate: request.request_estimate(),
					calibration_sample: false,
					cache_identity: None,
				},
			)
			.await;
		if let Err(error) = dispatched {
			if let Some(reservation) = reservation {
				reservation.abort().await?;
			}
			return Err(error);
		}
		let preparing = async {
			call.guard.suspend().await?;
			call.visibility.suspend().await?;
			store.start_inference(run, token, attempt).await
		}
		.await;
		if let Err(error) = preparing {
			store
				.complete_usage_record(
					run,
					attempt.0,
					&UsageOutcome::Rejected {
						class: "not_dispatched".into(),
					},
				)
				.await?;
			if let Some(reservation) = reservation {
				reservation.abort().await?;
			}
			return Err(error);
		}
		let progress =
			progress::ProgressBuffer::withholding_text(tokio::time::Instant::now(), true);
		let result = tokio::select! {
			result = provider.infer(request, &progress) => result,
			_ = self.environment.wait_for_inference_interruption(run.id, i64::MAX) => Err(Error::Invalid("MCP_SAMPLING_CANCELLED".into())),
			error = async { loop { tokio::time::sleep(INFERENCE_RECHECK_INTERVAL).await; if let Err(error) = call.guard.recheck_inference().await { break error; } } } => Err(error),
		};
		let outcome = match &result {
			Ok(response) => UsageOutcome::Completed(response.reported.clone()),
			Err(error) => match error.terminal_usage() {
				Some(response) => UsageOutcome::Completed(response.reported.clone()),
				None if matches!(
					error,
					Error::ProviderRejected { .. } | Error::ContextOverflow | Error::Plan(_)
				) =>
				{
					UsageOutcome::Rejected {
						class: "provider_rejected".into(),
					}
				}
				None => UsageOutcome::Unknown,
			},
		};
		store
			.complete_usage_record(run, attempt.0, &outcome)
			.await?;
		if let Some(reservation) = reservation {
			match &result {
				Ok(response) => reservation.settle(response).await?,
				Err(error) => match error.terminal_usage() {
					Some(response) => reservation.settle(response).await?,
					None if matches!(outcome, UsageOutcome::Rejected { .. }) => {
						reservation.abort().await?
					}
					None => {}
				},
			}
		}
		let resumed = async {
			call.visibility
				.resume()
				.await
				.map_err(|error| (InterruptionReason::StreamError, error))?;
			call.guard
				.resume()
				.await
				.map_err(|error| (InterruptionReason::Revoked, error))?;
			call.guard
				.inference()
				.await
				.map_err(|error| (InterruptionReason::Revoked, error))
		}
		.await;
		if let Err((reason, error)) = resumed {
			self.close_inference(run, token, attempt, ProgressOutcome::Interrupted(reason))
				.await?;
			store
				.invocation_finish(run, token, key, &json!({"mcp_error":"provider"}))
				.await?;
			return Err(error);
		}
		let (response, stop_reason) = match result {
			Ok(response) => (response, "endTurn"),
			Err(Error::TerminalResponse(
				aidash_domain::context::recovery::Failure::OutputTruncated,
				response,
			)) => (*response, "maxTokens"),
			Err(_) => {
				store
					.finish_inference(
						run,
						token,
						attempt,
						ProgressOutcome::Interrupted(InterruptionReason::StreamError),
					)
					.await?;
				store
					.invocation_finish(run, token, key, &json!({"mcp_error":"provider"}))
					.await?;
				return Err(Error::Invalid("MCP_SAMPLING_FAILED".into()));
			}
		};
		let value = json!({"role":"assistant","content":{"type":"text","text":response.text},"model":model.model_id,"stopReason":stop_reason});
		store
			.finish_sampling(run, token, key, &value, attempt)
			.await
	}
}

fn sampled(value: Value) -> Result<Value> {
	match value.get("mcp_error").and_then(Value::as_str) {
		Some("budget") => Err(Error::Invalid("MCP_SAMPLING_BUDGET".into())),
		Some(_) => Err(Error::Invalid("MCP_SAMPLING_FAILED".into())),
		None => Ok(value),
	}
}
