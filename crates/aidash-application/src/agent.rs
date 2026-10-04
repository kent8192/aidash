//! Durable agent execution use case. All effects cross application ports.
use crate::{Error, Result, execution::*, ports::execution::*};
use aidash_domain::{
	context::{self, Context, ContextEvent, ContextUsage},
	media::Selection,
	model::ModelConfig,
	semantic::{Failure, InputRead},
	tool::ToolConfig,
	*,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct Executor<'a> {
	environment: &'a dyn ExecutionEnvironment,
}
impl<'a> Executor<'a> {
	pub fn new(environment: &'a dyn ExecutionEnvironment) -> Self {
		Self { environment }
	}
	async fn tool_error(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &aidash_domain::provider::ToolCall,
		cursor: usize,
		message: String,
	) -> Result<()> {
		let mut context = run.context.clone();
		let event = ContextEvent::tool(call.clone(), json!({"error":message}));
		let growth = context::tool_event_growth(&context, &event);
		context.history.push(event);
		run.state.tool_mut()?.request_tokens =
			run.state.tool()?.request_tokens.saturating_add(growth);
		run.context = context.clone();
		run.state.tool_mut()?.cursor = cursor + 1;
		self.environment
			.store()
			.save_run(run, token, "run.tool_recorded")
			.await?;
		Ok(())
	}
	async fn tool_invocation_error(
		&self,
		run: &mut Run,
		token: Uuid,
		call: &aidash_domain::provider::ToolCall,
		cursor: usize,
		unfinished_key: Option<&str>,
		message: String,
	) -> Result<()> {
		if let Some(key) = unfinished_key {
			self.environment
				.store()
				.invocation_finish(run, token, key, &json!({"error":message}))
				.await?;
		}
		self.tool_error(run, token, call, cursor, message).await
	}
	async fn publish_model_text(
		&self,
		home: &dyn ExecutionHome,
		guard: Option<&dyn ExecutionAuthority>,
		run: &Run,
		worker: Uuid,
		text: &str,
	) -> Result<()> {
		if text.is_empty() {
			return Ok(());
		}
		if let Some(guard) = guard {
			guard
				.action("message.create", "workspace", run.workspace_id)
				.await?;
		}
		home.response_message(
			worker,
			run.included_input_seq(),
			&format!("{}:{}:output", run.id, run.state.tool()?.response_epoch),
			text,
		)
		.await
	}
	pub async fn advance(
		&self,
		run: &mut Run,
		token: Uuid,
		visibility: &mut dyn ExecutionVisibility,
	) -> Result<()> {
		let store = self.environment.store();
		let guard = self.environment.authority();
		let home = self.environment.home(run);

		// Accepted remote inputs remain deliverable even when the home task has
		// already reached a terminal state. Drain them before terminal recovery.
		self.environment.deliver_run_messages(run).await?;
		let task = home.task().await?;
		if task.status.is_terminal() {
			run.state = match task.status {
				TaskStatus::Completed => RunState::Completed(TerminalState {}),
				TaskStatus::Failed => RunState::Failed(TerminalState {}),
				TaskStatus::Cancelled | TaskStatus::Abandoned => {
					RunState::Cancelled(TerminalState {})
				}
				TaskStatus::Open
				| TaskStatus::Claimed
				| TaskStatus::Running
				| TaskStatus::Blocked => {
					return Err(Error::Conflict("task is not terminal".into()));
				}
			};
			let kind = if run.phase() == RunPhase::Failed {
				"run.failed"
			} else {
				"run.reconciled"
			};
			store.save_run(run, token, kind).await?;
			return Ok(());
		}
		if let RunState::Waiting(wait) = &run.state
			&& let WaitingState::FailureDelivery { target, .. } = wait.as_ref()
		{
			let target = *target;
			self.environment
				.transition_terminal_run_messages(run, target.task_status())
				.await?;
			run.state = target.into_state();
			store
				.save_run(
					run,
					token,
					if target == FailureTarget::Failed {
						"run.failed"
					} else {
						"run.cancelled"
					},
				)
				.await?;
			return Ok(());
		}
		run.recovery.retry = None;
		if std::mem::take(&mut run.recovery.lease_recovered) {
			let data = json!({"run_id":run.id,"task_id":run.task_id,"phase":run.phase(),"cause":"expired worker lease"});
			store
				.emit(
					home.local().then_some(run.workspace_id),
					"run.recovered",
					data.clone(),
				)
				.await?;
			home.report(
				&format!("{}:{}:recovered", run.id, run.revision),
				"remote.run.recovered",
				data,
			)
			.await?;
		}
		if run.control == RunControl::Cancelled {
			self.environment
				.transition_terminal_run_messages(run, TaskStatus::Cancelled)
				.await?;
			run.state = RunState::Cancelled(TerminalState {});
			store.save_run(run, token, "run.cancelled").await?;
			return Ok(());
		}
		let entry = self
			.environment
			.catalog()
			.get_for_run(&*run, &run.agent_id, &run.agent_version)
			.await?;
		let agent = self.environment.agent(&entry)?;
		// Separate phase poll frames to keep typed execution within the default worker stack.
		match run.state.clone() {
			RunState::Ready(_) => {
				let task = home.task().await?;
				if !task.dependencies.is_empty() {
					let mut waiting = false;
					for id in &task.dependencies {
						let dependency: Task = serde_json::from_value(
							home.read_record("task", &id.to_string()).await?,
						)?;
						if matches!(
							dependency.status,
							aidash_domain::TaskStatus::Failed
								| aidash_domain::TaskStatus::Cancelled
								| aidash_domain::TaskStatus::Abandoned
						) {
							return Err(Error::Invalid(format!(
								"dependency {} is {}",
								dependency.id, dependency.status
							)));
						}
						waiting |= dependency.status != aidash_domain::TaskStatus::Completed;
					}
					if waiting {
						run.state = RunState::Waiting(Box::new(WaitingState::Dependencies {
							wake_at: chrono::Utc::now() + chrono::Duration::seconds(2),
							resume: ReadyState {},
						}));
						store.save_run(run, token, "run.waiting").await?;
						return Ok(());
					}
				}
				home.claim(&task, &entry).await?;
				home.transition(aidash_domain::TaskStatus::Running).await?;
				run.state = RunState::Thinking(ThinkingState::default());
				store.save_run(run, token, "run.started").await?;
			}
			RunState::Thinking(thinking) => Box::pin(async {
				self.environment.reconcile_run_messages(run).await?;
				// An accepted remote correction is durable even if its first home
				// delivery failed. Deliver it before building any inference request.
				self.environment.deliver_run_messages(run).await?;
				let force_read_compaction = thinking.force_workspace_read_compaction;
				if let Some(guard) = guard {
					guard.inference().await?;
				}
				if run.step >= agent.max_steps {
					return Err(Error::Invalid("agent max_steps exceeded".into()));
				}
				let model_entry = self
					.environment
					.catalog().get_for_run(&*run, &agent.model.id, &agent.model.version)
					.await?;
				let model_cfg: ModelConfig = serde_json::from_value(model_entry.config)?;
				let window = model_cfg.context_window;
				let output_limit = model_cfg.output_token_limit();
				let model = self.environment.provider(model_cfg.clone())?;
				let tools = self.environment.tools(run, &entry).await?;
				let task = home.task().await?;
				if task.status == aidash_domain::TaskStatus::Completed {
					run.state = RunState::Completed(TerminalState {});
					store.save_run(run, token, "run.recovered").await?;
					return Ok(());
				}
				let mut instructions = context::agent_instructions("");
				for skill in &agent.skills {
					let entry = self
						.environment
						.catalog().get_for_run(&*run, &skill.id, &skill.version)
						.await?;
					instructions.push('\n');
					instructions.push_str(&format!("Skill {}@{}:\n", skill.id, skill.version));
					instructions.push_str(&self.environment.catalog().skill_instructions(&entry)?);
				}
				if tools.contains_key("skill_list") && home.has_local_authority()
				{
					instructions.push_str(&self.environment.skill_context(run).await?);
				}
				instructions.push_str("\nAdditional user instructions:\n");
				instructions.push_str(&agent.instructions);
				let documents =
					self.environment.documents(&entry).await?;
				let mut context = run.context.clone();
				capture_message_read_coverage(&mut context);
				let observation = home
					.observation(0, context::observation::DEFAULT_LIMIT)
					.await?;
				let inputs = store.run_inputs(run.id).await?;
				let input_seq = inputs
					.last()
					.map_or(run.observed_input_seq, |input| input.seq);
				let summary_seq = context.run_message_summary_seq;
				// Previous media-intake turns are retained as durable observations.
				// Repeating their text here can block a later media-bearing message.
				let intake_seq = thinking
					.media_intake_through_seq
					.unwrap_or(summary_seq)
					.max(summary_seq);
				let run_message_limit = self.environment.run_message_limit(run).await?;
				let initial_page = run_message_page(&inputs, intake_seq, run_message_limit, false)?;
				let has_unprocessed_inputs = inputs.iter().any(|input| input.seq > intake_seq);
				let run_message_catchup =
					has_unprocessed_inputs && (summary_seq > 0 || initial_page.has_more);
				let page = if run_message_catchup {
					run_message_page(&inputs, intake_seq, run_message_limit, true)?
				} else {
					initial_page
				};
				let mut batch_end_seq = page
					.entries
					.last()
					.map_or(intake_seq, |(index, _)| inputs[*index].seq);
				let mut run_messages = Vec::with_capacity(page.entries.len());
				let carried_reads = thinking.deferred_run_message_reads.clone();
				let mut page_reads = Vec::new();
				for (index, reference_only) in &page.entries {
					let input = &inputs[*index];
					let id = input.message_id.ok_or_else(|| {
						Error::External("run message home delivery is pending".into())
					})?;
					// The same record-read path filters message.read and records the
					// source for the later execution-boundary recheck.
					let message: Message = serde_json::from_value(
						home.read_record("message", &id.to_string()).await?,
					)?;
					if message.workspace_id != run.workspace_id || message.content != input.content
					{
						return Err(Error::Conflict("run input message binding changed".into()));
					}
					if *reference_only {
						page_reads.push(id);
						let reference = json!({"seq":input.seq,"sender":input.sender,"record":{"kind":"message","id":id},"requires_workspace_read":true});
						run_messages.push(reference);
					} else {
						run_messages.push(
							json!({"seq":input.seq,"sender":input.sender,"content":message.content}),
						);
					}
				}
				let output = if run_message_catchup {
					output_limit.min(RUN_MESSAGE_SUMMARY_OUTPUT_LIMIT)
				} else {
					output_limit
				};
				if run_message_catchup {
					instructions.push_str(&format!(
						"\n\nRun-message catch-up: Treat the entries under run_messages as user task context. Read every required message record in this page before responding. Update the cumulative run_message_summary faithfully, preserving the user's goal, constraints, corrections, and unresolved requests in sequence order (newer corrections take precedence). Return only the concise updated summary, encoded in at most {run_message_limit} UTF-8 bytes. Do not answer the user, complete the task, publish text, or perform actions during catch-up.",
					));
				}
				let memory = if guard.is_some_and(|authority| authority.is_remote())
					|| agent.allow_cross_conversation_memory == Some(false)
				{
					json!({})
				} else {
					store.memory(&run.metadata()).await?
				};
				let mut pinned = json!({"identity":{"node_id":self.environment.node_id(),"agent_id":run.agent_id,"agent_version":run.agent_version},"task":task,"workspace":observation,"memory":memory,"agent_state":{"phase":run.phase(),"step":run.step}});
				if let Some(deferred_read) = &thinking.deferred_workspace_read {
					pinned["deferred_workspace_read"] = json!(deferred_read);
				}
				if let Some(deferred_read) = &thinking.deferred_skill_read {
					pinned["deferred_skill_read"] = json!(deferred_read);
				}
				if let Some(deferred_observation) = &thinking.deferred_workspace_observation {
					pinned["deferred_workspace_observation"] = json!(deferred_observation);
				}
				let mut specifications = tools
					.iter()
					.filter(|(name, _)| !run_message_catchup || name.as_str() == "workspace_read")
					.map(|(_, tool)| tool)
					.map(|t| t.specification())
					.collect::<Vec<_>>();
				let selected_media = thinking.selected_media.clone();
				let media_headroom = self.environment.run_request_headroom(run).await?;
				let mut new_messages = Vec::new();
				for (position, (index, _)) in page.entries.iter().enumerate() {
					let input = &inputs[*index];
					if input.seq > context.media_inferred_seq {
						let id = input.message_id.ok_or_else(|| {
							Error::External("run message home delivery is pending".into())
						})?;
						let headroom = media_headroom.saturating_sub(
							encoded_run_message_reservation(&run_messages[..=position]),
						);
						new_messages.push((input.seq, id, headroom));
					}
				}
				let media = Box::pin(resolve_model_input_media(
					self.environment,
					run,
					guard,
					if run_message_catchup {
						&[]
					} else {
						&selected_media
					},
					&new_messages,
					media_headroom,
					&model_cfg,
				))
				.await?;
				if media.defer_human {
					let through = media.through_seq.ok_or_else(|| {
						Error::Invalid("run media batch cannot fit the model window".into())
					})?;
					run_messages.retain(|message| {
						message["seq"].as_i64().is_some_and(|seq| seq <= through)
					});
					page_reads.retain(|id| {
						page.entries.iter().any(|(index, reference_only)| {
							*reference_only
								&& inputs[*index].seq <= through
								&& inputs[*index].message_id == Some(*id)
						})
					});
					batch_end_seq = batch_end_seq.min(through);
				}
				let mut required_run_message_reads = carried_reads.clone();
				for id in page_reads {
					if !required_run_message_reads.contains(&id) {
						required_run_message_reads.push(id);
					}
				}
				let has_run_message_references = !required_run_message_reads.is_empty();
				if media.defer_human || media.defer_selected {
					specifications.clear();
					instructions.push_str("\nMedia intake is continuing. For this interim turn, postpone required workspace reads and the cumulative run-message summary. Preserve the user goals, constraints, and corrections in these run messages and describe the media in this request as plain text. Do not call tools or complete the task; deferred media will be provided in the next request.");
				}
				let context_window = window.saturating_sub(
					aidash_domain::provider::ModelRequest::content_parts_reservation(&media.parts),
				);
				let private_context = if agent.knowledge_digest.is_some() {
					json!({"reference_documents":documents.clone()})
				} else {
					serde_json::Value::Null
				};
				context::request_context_budget(
					window,
					output,
					&instructions,
					&specifications,
					&private_context,
				)?;
				let mut budget = context::RequestBudget {
					window: context_window,
					instructions: &instructions,
					tools: &specifications,
					max_output_tokens: output,
				};
				let minimum_request = budget
					.request(&Context::default(), &private_context)
					.estimated_total_tokens();
				budget.window = request_context_window(context_window, minimum_request);
				if force_read_compaction {
					budget.window =
						force_workspace_read_compaction_window(budget.window, minimum_request);
				}
				// Keep room for history and JSON message escaping. The final fitting
				// decision below measures the complete provider input, not this quota.
				let available = budget.remaining(&Context::default(), &private_context);
				let message_size = context::estimated_tokens(&json!(run_messages).to_string());
				let snapshot_fit = context::bound_snapshot(
					&mut pinned,
					available.saturating_sub(message_size) / 4,
				);
				if agent.knowledge_digest.is_some() {
					pinned["reference_documents"] = documents;
				}
				// The snapshot quota is a heuristic. Required IDs and other minimum
				// context may exceed it while the complete request still fits.
				// Keep the authorized run-directed input independent of the bounded
				// workspace preview. Admission has already capped its aggregate size.
				if !run_messages.is_empty() {
					pinned["run_messages"] = json!(run_messages);
				}
				if !carried_reads.is_empty() {
					pinned["deferred_run_message_reads"] = json!(carried_reads);
				}
				if has_run_message_references {
					pinned["run_message_read_instruction"] = if media.defer_human
						|| media.defer_selected
					{
						json!(
							"These run-message records still require workspace_read after media intake completes. Do not call tools in this interim request."
						)
					} else if run_message_catchup {
						json!(
							"Read every run_messages entry with requires_workspace_read and every deferred_run_message_reads ID through workspace_read(kind=message, id=record.id or the deferred ID) before returning the updated run_message_summary. Full content remains in each workspace record."
						)
					} else {
						json!(
							"Read every run_messages entry with requires_workspace_read and every deferred_run_message_reads ID through workspace_read(kind=message, id=record.id or the deferred ID) before completing the task. Full content remains in each workspace record."
						)
					};
				}
				if let Err(error) = snapshot_fit
					&& budget
						.request(&Context::default(), &pinned)
						.estimated_total_tokens()
						> budget.window
				{
					return Err(error.into());
				}
				let semantic_budget = budget.remaining(&Context::default(), &pinned) / 2;
				let semantic_inputs = inputs
					.iter()
					.filter_map(|input| {
						let message = run_messages.iter().find(|m| m["seq"] == input.seq)?;
						let text = message["content"].as_str()?;
						Some((
							InputRead {
								id: input.message_id?,
								sequence: input.seq,
								digest: self.environment.catalog().content_digest(text),
							},
							text.to_owned(),
						))
					})
					.collect::<Vec<_>>();
                if (guard.is_some() || home.local()) && let Some(semantic) = self.environment.semantic_context(run, &task, &semantic_inputs, semantic_budget, &entry).await? {
                    pinned["semantic_memory"] = json!(semantic);
                }
                let compactor = self.environment.compactor()?;
                compact_execution(&mut context, compactor.as_ref(), &budget, &pinned)
                    .await.map_err(|error| {
                        if guard.is_some_and(|guard| guard.is_remote()) && is_invalid(&error) {
                            Error::RemoteSemantic(Failure::ContextBudget)
                        } else { error }
                    })?;

				if let Some(guard) = guard {
					guard.inference().await?;
				}
				let mut request = budget.request(&context, &pinned);
				request.content_parts = media.parts;
				request.ensure_fits(window).map_err(Error::from).map_err(
					|error| {
						if guard.is_some_and(|guard| guard.is_remote()) {
							Error::RemoteSemantic(Failure::ContextBudget)
						} else {
							error
						}
					},
				)?;
				let request_tokens = request.estimated_total_tokens();
				let media_inferred_seq_before_response = context.media_inferred_seq;
				let observed_input_seq_before_response = run.observed_input_seq;
				let reservation = if let Some(guard) = guard {
					guard
						.reserve_inference(token, window, output, &request)
						.await?
				} else {
					None
				};
				// Race only inference, not replay-unsafe tools or durable transitions.
				// Release node-wide visibility and authorization row locks while the
				// provider waits; both boundaries are reacquired before accepting output.
				if let Some(guard) = guard {
					guard.suspend().await?;
				}
				visibility.suspend().await?;
				let result = tokio::select! {
					biased;
					cancelled = self.environment.wait_for_inference_cancellation(run.id) => match cancelled {
						Ok(()) => Err(Error::Conflict("run cancelled during inference".into())),
						Err(error) => Err(error),
					},
					result = model.infer(request) => result,
				};
				let resumed = visibility.resume().await;
				if let (Some(reservation), Ok(response)) = (reservation, result.as_ref()) {
					// Provider usage is billable even when authorization changed
					// or a transaction committed while its result was in flight.
					reservation.settle(response).await?;
				}
				resumed?;
				let result = result?;
				if let Some(guard) = guard {
					guard.resume().await?;
				}
				// Count only tool content that survived compaction and was present
				// in a successful provider request, not every completed read.
				capture_message_inference_coverage(&mut context);
				if let Some(seq) = media.through_seq {
					context.media_inferred_seq = context.media_inferred_seq.max(seq);
				}
				context.usage = Some(ContextUsage {
					input_tokens: result.input_tokens,
					output_tokens: result.output_tokens,
					context_window: window,
					compactions: context.compactions,
				});
				run.context = context.clone();
				let references_read_at_inference = required_run_message_reads
					.iter()
					.all(|id| referenced_message_inferred(&context, *id));
				if references_read_at_inference && !run_message_catchup {
					run.observed_input_seq = if media.defer_human {
						media.through_seq.unwrap_or(run.observed_input_seq)
					} else {
						input_seq
					};
				}
				run.state = RunState::ToolCall(Box::new(ToolCallState {
					response: result,
					response_epoch: response_epoch(run.revision, run.step),
					cursor: 0,
					included_input_seq: input_seq,
					request_window: budget.window,
					request_tokens,
					finalizing: false,
					media_inferred_seq_before_response,
					observed_input_seq_before_response,
					inferred_selected_media: if run_message_catchup || media.defer_selected {
						vec![]
					} else {
						selected_media.clone()
					},
					deferred_selected_media: if run_message_catchup || media.defer_selected {
						selected_media
					} else {
						vec![]
					},
					selected_media: vec![],
					deferred_human_media: media.defer_human || media.defer_selected,
					media_inferred_through_seq: media.through_seq,
					media_intake_through_seq: intake_seq,
					required_run_message_reads,
					references_read_at_inference,
					run_message_catchup,
					run_message_summary_end_seq: batch_end_seq,
					run_message_summary_limit: run_message_limit,
					deferred_reads: ThinkingState::default(),
					workspace_read_plan: None,
					skill_read_plan: None,
					workspace_observation_plan: None,
					workbench_approval_result: None,
				}));
				run.error = None;
				store.save_run(run, token, "model.completed").await?;
				Ok(())
			})
			.await?,
			RunState::ToolCall(_) => Box::pin(async {
				// An old home replica can still accept a correction directly from
				// an old executor. Wait for its upgraded database gate before any
				// model output or final completion crosses this boundary.
				match self.environment.require_terminal_safe_delivery(run).await {
					Ok(()) => {}
					Err(Error::Conflict(_)) => return Err(Error::TransactionPending),
					Err(error) => return Err(error),
				}
				// A preceding binary could have left a remote correction only on
				// the home node while this run was already awaiting finalization.
				self.environment.reconcile_run_messages(run).await?;
				let included_input_seq = run.included_input_seq();
				if store
					.run_inputs(run.id)
					.await?
					.last()
					.is_some_and(|input| input.seq > included_input_seq)
				{
					// This response predates an accepted correction. Discard its
					// text and calls before any effect crosses the tool boundary. Keep
					// the inference allowance and let the next stored response get a
					// fresh response_epoch for idempotency keys.
					let mut context = run.context.clone();
					let next_pending = stale_media_pending(
						&mut context,
						run.state.tool()?,
						&mut run.observed_input_seq,
					);
					run.context = context.clone();

					run.state = RunState::Thinking(next_pending);
					store.save_run(run, token, "run.message_received").await?;
					return Ok(());
				}
				let mut result = run.state.tool()?.response.clone();
				if run.state.tool()?.deferred_human_media {
					let mut context = run.context.clone();
					if result.text.trim().is_empty() {
						let next_pending = stale_media_pending(
							&mut context,
							run.state.tool()?,
							&mut run.observed_input_seq,
						);
						run.context = context.clone();

						run.step += 1;
						run.state = RunState::Thinking(next_pending);
						store
							.save_run(run, token, "run.media_observation_required")
							.await?;
						return Ok(());
					}
					record_media_observation(
						&mut context,
						&result.text,
						run.state.tool()?.media_inferred_through_seq,
						media_observation_budget(run.state.tool()?),
					);
					run.context = context.clone();

					run.step += 1;
					let pending = run.state.tool()?;
					run.state = RunState::Thinking(ThinkingState {
						selected_media: pending.deferred_selected_media.clone(),
						media_intake_through_seq: Some(
							pending
								.media_inferred_through_seq
								.unwrap_or(pending.media_intake_through_seq),
						),
						deferred_run_message_reads: pending.required_run_message_reads.clone(),
						..Default::default()
					});
					store.save_run(run, token, "run.media_deferred").await?;
					return Ok(());
				}
				let run_message_catchup = run.state.tool()?.run_message_catchup;
				if run_message_catchup {
					// A provider response cannot execute task tools during catch-up,
					// even if it returns calls that were not in the advertised tool set.
					result
						.tool_calls
						.retain(|call| call.name == "workspace_read");
				}
				let required_reads = run.state.tool()?.required_run_message_reads.clone();
				let mut context = run.context.clone();
				capture_message_read_coverage(&mut context);
				run.context = context.clone();
				let references_read = required_reads
					.iter()
					.all(|id| referenced_message_read(&context, *id));
				let references_inferred = required_reads
					.iter()
					.all(|id| referenced_message_inferred(&context, *id));
				let informed_response = required_reads.is_empty()
					|| (references_read
						&& references_inferred
						&& run.state.tool()?.references_read_at_inference);
				let cursor = run.state.tool()?.cursor;
				if !informed_response
					&& let Some(call) = result.tool_calls.get(cursor)
					&& !is_required_message_read(call, &required_reads, &context)
				{
					context.history.push(ContextEvent::RunMessageReadRequired {
						message_ids: required_reads.clone(),
					});

					run.step += 1;
					run.state = RunState::Thinking(stale_media_pending(
						&mut context,
						run.state.tool()?,
						&mut run.observed_input_seq,
					));
					run.context = context.clone();
					store
						.save_run(run, token, "run.message_read_required")
						.await?;
					return Ok(());
				}
				if cursor >= result.tool_calls.len() && !informed_response {
					context.history.push(ContextEvent::RunMessageReadRequired {
						message_ids: required_reads.clone(),
					});

					run.step += 1;
					run.state = RunState::Thinking(stale_media_pending(
						&mut context,
						run.state.tool()?,
						&mut run.observed_input_seq,
					));
					run.context = context.clone();
					store
						.save_run(run, token, "run.message_read_required")
						.await?;
					return Ok(());
				}
				if cursor >= result.tool_calls.len() && run_message_catchup {
					let summary = result.text.trim().to_owned();
					if summary.is_empty() {
						context
							.history
							.push(ContextEvent::RunMessageSummaryRequired {
								through_seq: run.state.tool()?.run_message_summary_end_seq,
								max_bytes: None,
								reason: None,
							});

						run.step += 1;
						run.state = RunState::Thinking(stale_media_pending(
							&mut context,
							run.state.tool()?,
							&mut run.observed_input_seq,
						));
						run.context = context.clone();
						store
							.save_run(run, token, "run.message_summary_required")
							.await?;
						return Ok(());
					}
					let summary_limit = run.state.tool()?.run_message_summary_limit;
					if summary.len() > summary_limit {
						context
							.history
							.push(ContextEvent::RunMessageSummaryRequired {
								through_seq: run.state.tool()?.run_message_summary_end_seq,
								max_bytes: Some(summary_limit),
								reason: Some("summary exceeded the complete-summary limit".into()),
							});

						run.step += 1;
						run.state = RunState::Thinking(stale_media_pending(
							&mut context,
							run.state.tool()?,
							&mut run.observed_input_seq,
						));
						run.context = context.clone();
						store
							.save_run(run, token, "run.message_summary_required")
							.await?;
						return Ok(());
					}
					context.run_message_summary = summary;
					context.run_message_summary_seq = run.state.tool()?.run_message_summary_end_seq;
					let summarized_ids = required_reads
						.iter()
						.copied()
						.collect::<std::collections::BTreeSet<_>>();
					context.history.retain(|event| {
						message_read_range(event)
							.is_none_or(|(id, _, _, _)| !summarized_ids.contains(&id))
					});
					for id in &required_reads {
						context.message_read_coverage.remove(id);
						context.message_inference_coverage.remove(id);
					}
					run.context = context.clone();

					run.state = RunState::Thinking(ThinkingState {
						selected_media: pending_selected_media(run.state.tool()?),
						..Default::default()
					});
					store.save_run(run, token, "run.message_summarized").await?;
					return Ok(());
				}
				if !run_message_catchup && !result.tool_calls.is_empty() && informed_response {
					self.publish_model_text(home.as_ref(), guard, run, token, &result.text)
						.await?;
				}
				if cursor >= result.tool_calls.len() {
					if result.tool_calls.is_empty() {
						if !run.state.tool()?.deferred_selected_media.is_empty() {
							if !result.text.trim().is_empty() {
								let mut context = run.context.clone();
								record_media_observation(
									&mut context,
									&result.text,
									None,
									media_observation_budget(run.state.tool()?),
								);
								run.context = context.clone();
							}

							run.step += 1;
							run.state = RunState::Thinking(ThinkingState {
								selected_media: run.state.tool()?.deferred_selected_media.clone(),
								..Default::default()
							});
							store.save_run(run, token, "run.media_deferred").await?;
							return Ok(());
						}
						let children = home.child_summary(run.task_id).await?;
						if children.has_pending {
							self.publish_model_text(home.as_ref(), guard, run, token, &result.text)
								.await?;
							let failed = children.has_failed;
							if failed {
								if let Some(guard) = guard {
									guard.action("human.request", "run", run.id).await?;
								}
								let response_epoch = run.state.tool()?.response_epoch;
								let h=store.human_request(run,"INFORMATION_REQUEST","A subtask needs intervention. You can explicitly abandon failed, blocked or cancelled subtasks in their task details, providing a reason. Then answer this request to continue with the remaining results, or cancel this parent.",&format!("{}:{}:subtasks",run.id,response_epoch)).await?;
								run.state = RunState::Waiting(Box::new(WaitingState::Human {
									request_id: h.id,
									resume: ResumeState::Thinking(ThinkingState::default()),
								}));
							} else {
								run.state = RunState::Waiting(Box::new(WaitingState::Children {
									wake_at: chrono::Utc::now() + chrono::Duration::seconds(2),
									resume: ThinkingState::default(),
								}));
							}
							run.step += 1;

							store.save_run(run, token, "run.waiting").await?;
							return Ok(());
						}
						let artifact = ArtifactInput {
							kind: "text".into(),
							name: result_artifact_name(&home.task().await?.title),
							content: json!(result.text),
						};
						if let Some(guard) = guard {
							guard
								.action("artifact.create", "artifact", run.task_id)
								.await?;
							guard.action("task.complete", "task", run.task_id).await?;
						}
						if !store.begin_final_completion(run, token).await? {
							run.state = RunState::Thinking(ThinkingState::default());

							store.save_run(run, token, "run.message_received").await?;
							return Ok(());
						}
						self.publish_model_text(home.as_ref(), guard, run, token, &result.text)
							.await?;
						if let Err(error) = home
							.complete(&format!("{}:complete", run.id), &artifact)
							.await
						{
							if matches!(error, Error::Conflict(_) | Error::Domain(aidash_domain::Error::Conflict(_)))
								&& home.child_summary(run.task_id).await?.has_pending
							{
								run.step += 1;

								run.state = RunState::Waiting(Box::new(WaitingState::Children {
									wake_at: chrono::Utc::now() + chrono::Duration::seconds(2),
									resume: ThinkingState::default(),
								}));
								store.save_run(run, token, "run.waiting").await?;
								return Ok(());
							}
							return Err(error);
						}
						run.state = RunState::Completed(TerminalState {});

						store.save_run(run, token, "run.completed").await?;
					} else {
						run.step += 1;
						let mut next = run.state.tool()?.deferred_reads.clone();
						next.selected_media = pending_selected_media(run.state.tool()?);
						run.state = RunState::Thinking(next);
						store.save_run(run, token, "run.thinking").await?;
					}
					return Ok(());
				}
				let mut context = run.context.clone();
				capture_message_read_coverage(&mut context);
				let mut call = result.tool_calls[cursor].clone();
				if !pending_selected_media(run.state.tool()?).is_empty()
					&& !read_only_after_model_media_selection(&call.name)
				{
					return self
						.tool_error(
							run,
							token,
							&call,
							cursor,
							"selected model media must be inferred before this tool call; retry it after the next model response".into(),
						)
						.await;
				}
				let mut prepared_result = None;
				if call.name == "workspace_read" {
					let (read_range, saved_read) = match workspace_read_plan_result(
						&call,
						run.step,
						cursor,
						run.state.tool()?,
					) {
						Ok(plan) => plan,
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
							return self.tool_error(run, token, &call, cursor, message).await;
						}
						Err(error) => return Err(error),
					};
					if let Some(output) = saved_read {
						prepared_result = Some(output.clone());
					} else {
						let request_tokens = run.state.tool()?.request_tokens;
						let request_window = run.state.tool()?.request_window;
						match cap_workspace_read(
							home.as_ref(),
							&context,
							&call,
							read_range,
							request_tokens,
							request_window,
							result.tool_calls.len().saturating_sub(cursor + 1),
						)
						.await
						{
							Ok(WorkspaceReadFit::Skip) => {}
							Ok(WorkspaceReadFit::NoEnvelopeRoom) => {
								// Start a new inference turn without appending an error event:
								// even that envelope could exceed the smaller forced-compaction
								// quota on the next request.

								if !run_message_catchup {
									run.step += 1;
								}
								run.state = RunState::Thinking(ThinkingState {
									force_workspace_read_compaction: true,
									deferred_workspace_read: Some(deferred_workspace_read(&call)),
									selected_media: pending_selected_media(run.state.tool()?),
									..Default::default()
								});
								store.save_run(run, token, "run.read_deferred").await?;
								return Ok(());
							}
							Ok(WorkspaceReadFit::Fitted {
								call: bounded,
								result: output,
							}) => {
								call = bounded;
								result.tool_calls[cursor] = call.clone();
								run.state.tool_mut()?.response = result.clone();
								run.state.tool_mut()?.workspace_read_plan = Some(ReadPlan {
									step: run.step,
									cursor,
									call: call.clone(),
									result: output.clone(),
								});
								prepared_result = Some(output);
							}
							Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
								return self.tool_error(run, token, &call, cursor, message).await;
							}
							Err(error) => return Err(error),
						}
					}
				}
				if call.name == "skill_read" && call.arguments.get("skill").is_some() {
					let range = match skill_read_range(&call) {
						Ok(range) => range,
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
							return self.tool_error(run, token, &call, cursor, message).await;
						}
						Err(error) => return Err(error),
					};
					let saved_output = run
						.state
						.tool()?
						.skill_read_plan
						.as_ref()
						.filter(|p| p.step == run.step && p.cursor == cursor && p.call == call)
						.map(|p| p.result.clone());
					if let Some(output) = saved_output {
						prepared_result = Some(output);
					} else {
						let output = match self.environment.builtins(run).await?
							.get("skill_read")
							.ok_or_else(|| Error::Invalid("skill_read builtin unavailable".into()))?
							.invoke(run, call.arguments.clone(), "")
							.await
						{
							Ok(output) => output,
							Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
								return self.tool_error(run, token, &call, cursor, message).await;
							}
							Err(error) => return Err(error),
						};
						let request_tokens = run.state.tool()?.request_tokens;
						let request_window = run.state.tool()?.request_window;
						let budget = WorkspaceReadFitBudget {
							requested: range.requested,
							offset: range.offset,
							request_tokens,
							request_window,
							remaining_calls: result.tool_calls.len().saturating_sub(cursor + 1),
						};
						let Some(chars) = fit_skill_read_chars(&context, &call, &output, budget)
						else {
							run.step += 1;
							run.state = RunState::Thinking(ThinkingState {
								force_workspace_read_compaction: true,
								deferred_skill_read: Some(deferred_skill_read(&call)),
								selected_media: pending_selected_media(run.state.tool()?),
								..Default::default()
							});
							store
								.save_run(run, token, "run.skill_read_deferred")
								.await?;
							return Ok(());
						};
						call.arguments["max_chars"] = json!(chars);
						result.tool_calls[cursor] = call.clone();
						run.state.tool_mut()?.response = result.clone();
						let bounded = skill_read_result(&output, chars);
						run.state.tool_mut()?.skill_read_plan = Some(ReadPlan {
							step: run.step,
							cursor,
							call: call.clone(),
							result: bounded.clone(),
						});
						prepared_result = Some(bounded);
					}
				}
				if call.name == "workspace_observe" {
					let saved_output = run
						.state
						.tool()?
						.workspace_observation_plan
						.as_ref()
						.filter(|p| p.step == run.step && p.cursor == cursor && p.call == call)
						.map(|p| p.result.clone());
					if let Some(output) = saved_output {
						prepared_result = Some(output);
					} else {
						let offset = call.arguments["offset"].as_u64().unwrap_or(0) as usize;
						let requested = call.arguments["limit"]
							.as_u64()
							.unwrap_or(context::observation::DEFAULT_LIMIT as u64)
							as usize;
						let request_tokens = run.state.tool()?.request_tokens;
						let request_window = run.state.tool()?.request_window;
						let remaining_calls = result.tool_calls.len().saturating_sub(cursor + 1);
						let fitted = home
							.observation_fitted(offset, requested, &|limit, output| {
								Ok(workspace_observation_event_fits(
									&context,
									&call,
									limit,
									output,
									request_tokens,
									request_window,
									remaining_calls,
								))
							})
							.await?;
						let Some((limit, output)) = fitted else {
							// Retry after compaction without appending an observation event
							// that cannot fit in the next request quota.

							run.step += 1;
							run.state = RunState::Thinking(ThinkingState {
								force_workspace_read_compaction: true,
								deferred_workspace_observation: Some(
									deferred_workspace_observation(&call),
								),
								selected_media: pending_selected_media(run.state.tool()?),
								..Default::default()
							});
							store
								.save_run(run, token, "run.observation_deferred")
								.await?;
							return Ok(());
						};
						call.arguments["limit"] = json!(limit);
						result.tool_calls[cursor] = call.clone();
						run.state.tool_mut()?.response = result.clone();
						run.state.tool_mut()?.workspace_observation_plan = Some(ReadPlan {
							step: run.step,
							cursor,
							call: call.clone(),
							result: output.clone(),
						});
						prepared_result = Some(output);
					}
				}
				let call = &call;
				let tools = self.environment.tools(run, &entry).await?;
				let Some(tool) = tools.get(&call.name) else {
					return self
						.tool_error(
							run,
							token,
							call,
							cursor,
							format!("unavailable tool {}", call.name),
						)
						.await;
				};
				if let Some(guard) = guard {
					match guard.tool(call).await {
						Ok(()) => {}
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
							return self.tool_error(run, token, call, cursor, message).await;
						}
						Err(error) => return Err(error),
					}
				}
				let response_epoch = run.state.tool()?.response_epoch;
				let key = format!("{}:{}:{}", run.id, response_epoch, cursor);
				// New workbench versions require an explicit, one-call approval for
				// external writes. Legacy versions have no behavior flags and keep
				// their existing execution contract.
				if agent.allow_task_creation.is_some()
					&& let Some(index) = call
						.name
						.strip_prefix("plugin_")
						.and_then(|value| value.parse::<usize>().ok())
					&& let Some(reference) = agent.tools.get(index)
				{
					let entry = self
						.environment
						.catalog().get_for_run(&*run, &reference.id, &reference.version)
						.await?;
					let config: ToolConfig = serde_json::from_value(entry.config)?;
					let writes = matches!(config, ToolConfig::Http { ref replay, .. } | ToolConfig::Mcp { ref replay, .. } if replay != "read_only");
					if writes {
						if let Some(decision) = run
							.state
							.tool()?
							.workbench_approval_result
							.as_ref()
							.filter(|d| d.key == key && d.call == *call)
						{
							if !decision.approved || decision.expires_at <= chrono::Utc::now() {
								return self
									.tool_error(
										run,
										token,
										call,
										cursor,
										"external write approval was denied or expired".into(),
									)
									.await;
							}
						} else {
							let prompt = format!(
								"Approve this exact external tool action once? Tool: {}@{}; call: {}",
								reference.id,
								reference.version,
								serde_json::to_string(call)?
							);
							let request = store
								.human_request(
									run,
									"APPROVAL_REQUIRED",
									&prompt,
									&format!("{key}:workbench-approval"),
								)
								.await?;
							run.state =
								RunState::Waiting(Box::new(WaitingState::ExternalApproval {
									request_id: request.id,
									key: key.clone(),
									call: call.clone(),
									expires_at: request.created_at + chrono::Duration::minutes(15),
									resume: Box::new(run.state.tool()?.clone()),
								}));
							store
								.save_run(run, token, "run.waiting_for_tool_approval")
								.await?;
							return Ok(());
						}
					}
				}
				let invocation = store
					.invocation_start(
						run,
						token,
						&key,
						&call.name,
						&call.arguments,
						tool.replay_safe(),
					)
					.await?;
				if invocation.status == "UNCERTAIN" {
					if let Some(guard) = guard {
						guard.action("human.request", "run", run.id).await?;
					}
					store.reconciliation_request(run, token, &key, &format!("Tool {} may have completed before the worker stopped. Reconcile the external effect, then answer with a JSON object containing result. It will not be executed again. Invocation: {key}",call.name)).await?;
					store.save_run(run, token, "run.waiting").await?;
					return Ok(());
				}
				let unfinished_key = (invocation.status != "COMPLETED").then_some(key.as_str());
				let output = if invocation.status == "COMPLETED" {
					invocation.result.ok_or_else(|| {
						Error::Conflict("completed invocation has no result".into())
					})?
				} else if let Some(output) = prepared_result {
					output
				} else {
					match tool.invoke(run, call.arguments.clone(), &key).await {
						Ok(output) => output,
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => json!({"error":message}),
						Err(e) => return Err(e),
					}
				};
				if call.name == "file_read"
					&& call.arguments["representation"] == "model_input"
					&& output["status"] == "completed"
					&& output["metadata"]["file_id"] == call.arguments["file_id"]
				{
					let selection: Selection =
						serde_json::from_value(
							json!({"file_id":call.arguments["file_id"],"expected_digest":call.arguments["expected_digest"]}),
						)?;
					let mut selected = run.state.tool()?.selected_media.clone();
					let deferred_count = run.state.tool()?.deferred_selected_media.len();
					if selected.len() + deferred_count >= 8 {
						return self
							.tool_invocation_error(
								run,
								token,
								call,
								cursor,
								unfinished_key,
								"model media input exceeds count limit".into(),
							)
							.await;
					}
					selected.push(selection);
					let mut selections = run.state.tool()?.deferred_selected_media.clone();
					selections.extend(selected.iter().cloned());
					let Some(guard) = guard else {
						return self
							.tool_invocation_error(
								run,
								token,
								call,
								cursor,
								unfinished_key,
								"model media input requires scoped file access".into(),
							)
							.await;
					};
					match Box::pin(guard.model_media(&selections)).await {
						Ok(parts) => {
							let headroom = self.environment.run_request_headroom(run).await?;
							let model_entry = self
								.environment
								.catalog().get_for_run(&*run, &agent.model.id, &agent.model.version)
								.await?;
							let model: ModelConfig = serde_json::from_value(model_entry.config)?;
							match check_model_media_headroom(headroom, parts, &model) {
								Ok(()) => {}
								Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
									return self
										.tool_invocation_error(
											run,
											token,
											call,
											cursor,
											unfinished_key,
											message,
										)
										.await;
								}
								Err(error) => return Err(error),
							}
						}
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => {
							return self
								.tool_invocation_error(
									run,
									token,
									call,
									cursor,
									unfinished_key,
									message,
								)
								.await;
						}
						Err(error) => return Err(error),
					}
					run.state.tool_mut()?.selected_media = selected;
				}
				if invocation.status != "COMPLETED" {
					store.invocation_finish(run, token, &key, &output).await?;
				}
				let event = ContextEvent::tool(call.clone(), output.clone());
				record_message_read(&mut context, &event);
				let growth = context::tool_event_growth(&context, &event);
				context.history.push(event);
				run.state.tool_mut()?.request_tokens =
					run.state.tool()?.request_tokens.saturating_add(growth);
				run.context = context.clone();
				let progress = run.state.tool_mut()?;
				progress.workspace_read_plan = None;
				progress.skill_read_plan = None;
				progress.workspace_observation_plan = None;
				progress.cursor = cursor + 1;
				match FrameworkResult::decode(call, &output)? {
					FrameworkResult::Approval(approval_id) => {
						run.step += 1;
						run.state = RunState::Waiting(Box::new(WaitingState::CoreApproval {
							approval_id,
							resume: ThinkingState::default(),
						}));
					}
					FrameworkResult::Human(request_id) => {
						run.step += 1;
						run.state = RunState::Waiting(Box::new(WaitingState::Human {
							request_id,
							resume: ResumeState::Thinking(ThinkingState::default()),
						}));
					}
					FrameworkResult::Wait(seconds) => {
						run.suspend(|resume| WaitingState::Timer {
							wake_at: chrono::Utc::now() + chrono::Duration::seconds(seconds),
							resume,
						})?
					}
					FrameworkResult::Ordinary => {}
				}
				home.report(
					&format!("{key}:tool"),
					"remote.tool.completed",
					json!({"run_id":run.id,"call":call,"result":output}),
				)
				.await?;
				store.save_run(run, token, "run.tool_recorded").await?;
				Ok(())
			})
			.await?,
			RunState::Waiting(waiting) => {
				let mut waiting = *waiting;
				if let Some(id) = waiting.request_id() {
					if let Some(guard) = guard {
						guard.human_read(id).await?;
					}
                    let h = store.human_request_by_id(id).await?;
					let response = if matches!(&waiting, WaitingState::ExternalApproval {expires_at,..} if *expires_at <= chrono::Utc::now()) {store.expire_workbench_approval(id).await?.response} else {h.response}.ok_or_else(||Error::Conflict("human request has not been answered".into()))?;
					match &mut waiting {
						WaitingState::ExternalApproval {
							key,
							call,
							expires_at,
							resume,
							..
						} => {
							#[derive(serde::Deserialize)]
							struct ApprovalAnswer {
								approved: bool,
							}
							let decision =
								serde_json::from_value::<ApprovalAnswer>(response.clone()).ok();
							resume.workbench_approval_result = Some(ApprovalDecision {
								key: key.clone(),
								call: call.clone(),
								approved: decision.is_some_and(|d| d.approved),
								expires_at: *expires_at,
							})
						}
						WaitingState::Reconciliation { key, .. } => {
							#[derive(serde::Deserialize)]
							struct ReconciliationAnswer {
								#[serde(deserialize_with = "aidash_domain::required_json")]
								result: Value,
							}
							let answer: ReconciliationAnswer =
								serde_json::from_value(response.clone()).map_err(|_| {
									Error::Invalid(
										"reconciliation response must contain result".into(),
									)
								})?;
							store
								.invocation_finish(run, token, key, &answer.result)
								.await?;
						}
						_ => {}
					}
					if !matches!(waiting, WaitingState::Reconciliation { .. }) {
						run.context.history.push(ContextEvent::Human {
							request: h.prompt,
							request_kind: h.kind,
							response,
						});
					}
				}
				run.state = match waiting {
					WaitingState::Dependencies { resume, .. } => RunState::Ready(resume),
					WaitingState::Children { resume, .. }
					| WaitingState::CoreApproval { resume, .. } => RunState::Thinking(resume),
					WaitingState::Timer { resume, .. } | WaitingState::Human { resume, .. } => {
						resume.into_state()
					}
					WaitingState::ExternalApproval { resume, .. }
					| WaitingState::Reconciliation { resume, .. } => RunState::ToolCall(resume),
					WaitingState::FailureDelivery { .. } => {
						return Err(Error::Invalid(
							"failure delivery requires terminal handling".into(),
						));
					}
				};
				store.save_run(run, token, "run.resumed").await?;
			}
			RunState::Completed(_) | RunState::Failed(_) | RunState::Cancelled(_) => {
				return Err(Error::Conflict("run is not executable".into()));
			}
		}
		Ok(())
	}
}
async fn cap_workspace_read(
	home: &dyn ExecutionHome,
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	range: WorkspaceReadRange,
	request_tokens: usize,
	request_window: usize,
	remaining_calls: usize,
) -> Result<WorkspaceReadFit> {
	let WorkspaceReadRange { offset, requested } = range;
	let (Some(kind), Some(id)) = (
		call.arguments["kind"].as_str(),
		call.arguments["id"].as_str(),
	) else {
		return Ok(WorkspaceReadFit::Skip);
	};
	let output = home.read_record_chunk(kind, id, offset, requested).await?;
	let Some(bounded) = fit_workspace_read_chars(
		context,
		call,
		&output,
		WorkspaceReadFitBudget {
			requested,
			offset,
			request_tokens,
			request_window,
			remaining_calls,
		},
	)?
	else {
		return Ok(WorkspaceReadFit::NoEnvelopeRoom);
	};
	let mut bounded_call = call.clone();
	bounded_call.arguments["max_chars"] = json!(bounded);
	Ok(WorkspaceReadFit::Fitted {
		call: bounded_call,
		result: workspace_read_result(&output, requested, offset, bounded),
	})
}
async fn resolve_model_input_media(
	environment: &dyn ExecutionEnvironment,
	run: &Run,
	guard: Option<&dyn ExecutionAuthority>,
	selections: &[Selection],
	messages: &[(i64, Uuid, usize)],
	headroom: usize,
	model: &ModelConfig,
) -> Result<ResolvedMedia> {
	let selected_parts = if selections.is_empty() {
		Vec::new()
	} else {
		guard
			.ok_or(Error::Forbidden)?
			.model_media(selections)
			.await?
	};
	let has_human_media = environment
		.store()
		.run_message_has_media(&messages.iter().map(|(_, id, _)| *id).collect::<Vec<_>>())
		.await?;
	let human = if !has_human_media {
		HumanMediaBatch {
			parts: Vec::new(),
			through_seq: messages.last().map(|(seq, _, _)| *seq),
			has_more: false,
		}
	} else if let Some(guard) = guard {
		guard.human_message_media(messages, model).await?
	} else {
		environment
			.operator_human_message_media(run, messages, model)
			.await?
	};
	let batch_headroom = human
		.through_seq
		.and_then(|through| {
			messages
				.iter()
				.find(|(seq, _, _)| *seq == through)
				.map(|(_, _, available)| *available)
		})
		.unwrap_or(headroom);
	let (parts, defer_selected) = choose_inference_media(
		selected_parts,
		human.parts,
		batch_headroom,
		human.through_seq.is_some(),
		model,
	);
	if !model.has_current_media_route_for_parts(&parts) {
		return Err(Error::MediaRouteUnavailable(model.model_id.clone()));
	}
	Ok(ResolvedMedia {
		parts,
		through_seq: human.through_seq,
		defer_selected,
		defer_human: human.has_more,
	})
}
struct ResolvedMedia {
	parts: Vec<aidash_domain::provider::ContentPart>,
	through_seq: Option<i64>,
	defer_selected: bool,
	defer_human: bool,
}
enum WorkspaceReadFit {
	Skip,
	NoEnvelopeRoom,
	Fitted {
		call: aidash_domain::provider::ToolCall,
		result: Value,
	},
}
fn is_invalid(error: &Error) -> bool {
	matches!(
		error,
		Error::Invalid(_) | Error::Domain(aidash_domain::Error::Invalid(_))
	)
}
async fn compact_execution(
	context: &mut Context,
	classifier: &dyn crate::ports::CompactionClassifier,
	budget: &context::RequestBudget<'_>,
	pinned: &Value,
) -> Result<()> {
	let mut candidate = context.clone();
	context::observation::normalize_history(&mut candidate.history);
	crate::context::compact(&mut candidate, classifier, budget, pinned).await?;
	*context = candidate;
	Ok(())
}

#[cfg(test)]
mod tests;
