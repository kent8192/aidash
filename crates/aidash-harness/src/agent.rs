//! Durable agent execution use case. All effects cross application ports.
use aidash_application::{
	Error, Result,
	capabilities::approvals::{
		AUTO_MODE_INSTRUCTION, DONT_ASK_HUMAN_REQUEST, INVOKE, ToolCallApproval, ToolCallRequest,
		ToolCallTarget, answer_deadline_response, answer_for_model,
	},
	execution::*,
	ports::execution::*,
};
use aidash_domain::{
	approval_rules::ApprovalMode,
	context::{self, Context, ContextEvent, ContextUsage},
	exposure,
	media::ModelMedia,
	model::ModelConfig,
	projection::{ProjectionVersion, PromptCache},
	provider::estimate::{EstimateSnapshot, RequestEstimator},
	provider::progress::{InferenceAttemptId, InterruptionReason, ProgressOutcome},
	provider::usage::{ShadowEstimate, UsageDispatch, UsageOutcome, UsagePurpose},
	registry::bindings::{AgentBindings, BindingSnapshot},
	research::citations,
	semantic::{Failure, InputRead},
	tool::{Continuation, ResultFitting, ToolIdentity, ToolUseMode, Yield},
	*,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// The calls of one response that a Tool Batch may cover, starting at the cursor.
struct BatchStep<'a> {
	home: &'a dyn ExecutionHome,
	guard: Option<&'a dyn ExecutionAuthority>,
	tools: &'a Tools,
	calls: &'a [aidash_domain::provider::ToolCall],
	cursor: usize,
	end: usize,
}

mod context_sources;
use context_sources::{cached_data_phase, collect_data_phase, read_planned_source};

pub struct Executor<'a> {
	environment: &'a dyn ExecutionEnvironment,
}
impl<'a> Executor<'a> {
	async fn tool_delivery_result(
		store: &dyn ExecutionStore,
		run: &Run,
		token: Uuid,
		key: &str,
		result: Result<Value>,
	) -> Result<Value> {
		match result {
			Ok(value) => Ok(value),
			Err(
				Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message)),
			) => Ok(json!({"error":message})),
			Err(Error::ToolNotDelivered {
				message,
				retry_after,
			}) => {
				let failure = json!({"error":{"kind":"not_delivered","message":message}});
				let count = store
					.invocation_not_delivered(run, token, key, &failure)
					.await?;
				if count > aidash_domain::semantic::OUTAGE_RETRIES {
					Ok(failure)
				} else {
					Err(Error::ToolRetry {
						message,
						count,
						retry_after,
					})
				}
			}
			Err(error @ Error::External(_)) => {
				store.invocation_answer_lost(run, token, key).await?;
				Err(error)
			}
			Err(error) => Err(error),
		}
	}
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
		context.push(event);
		run.state.tool_mut()?.request_tokens =
			run.state.tool()?.request_tokens.saturating_add(growth);
		run.context = context.clone();
		run.state.tool_mut()?.advance(cursor + 1);
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
	/// Execute the Tool Batch `[cursor, end)` and adopt its results in call
	/// order. Returns `false`, having persisted nothing, when authority checks
	/// shrink a fresh plan below two calls; the cursor call then runs alone.
	async fn execute_batch(&self, run: &mut Run, token: Uuid, step: BatchStep<'_>) -> Result<bool> {
		let BatchStep {
			home,
			guard,
			tools,
			calls,
			cursor,
			mut end,
		} = step;
		let store = self.environment.store();
		let resumed = run.state.tool()?.batch_end.is_some();
		if end <= cursor || end > calls.len() {
			return Err(Error::Conflict(
				"Tool Batch exceeds its model response".into(),
			));
		}
		let tool = |call: &aidash_domain::provider::ToolCall| {
			tools.get(&call.name).cloned().ok_or_else(|| {
				Error::Conflict(format!("batched tool {} is unavailable", call.name))
			})
		};
		// Current authority precedes the journal, as on the sequential path. A
		// resumed batch is already journaled: a revoked call records its denial.
		let mut denied = vec![None; end - cursor];
		if let Some(guard) = guard {
			for index in cursor..end {
				let call = &calls[index];
				match guard.tool(call, &tool(call)?.contract()).await {
					Ok(()) => {}
					Err(
						Error::Invalid(message)
						| Error::Domain(aidash_domain::Error::Invalid(message)),
					) => {
						if !resumed {
							end = index;
							break;
						}
						denied[index - cursor] = Some(message);
					}
					Err(error) => return Err(error),
				}
			}
		}
		if end - cursor < 2 {
			return Ok(false);
		}
		denied.truncate(end - cursor);
		let response_epoch = run.state.tool()?.response_epoch;
		let keys: Vec<String> = (cursor..end)
			.map(|index| format!("{}:{response_epoch}:{index}", run.id))
			.collect();
		run.state.tool_mut()?.batch_end = Some(end);
		let journal: Vec<BatchInvocation<'_>> = (cursor..end)
			.map(|index| BatchInvocation {
				key: &keys[index - cursor],
				name: &calls[index].name,
				input: &calls[index].arguments,
			})
			.collect();
		let outcomes = store.invocation_start_batch(run, token, &journal).await?;
		if outcomes.len() != end - cursor {
			return Err(Error::Conflict("Tool Batch admission lost a call".into()));
		}
		let mut outputs: Vec<Option<Value>> = vec![None; end - cursor];
		let mut dispatch = Vec::new();
		// Admit every call before dispatching any, so checks that need the Run's
		// authority scope exclusively never wait behind a sibling's execution.
		for (offset, outcome) in outcomes.into_iter().enumerate() {
			let call = &calls[cursor + offset];
			let denial = denied[offset].take();
			match outcome.status.as_str() {
				// As on the sequential path, a revoked call's journaled result
				// never crosses into the Context Journal.
				"COMPLETED" => {
					let result = outcome.result.ok_or_else(|| {
						Error::Conflict("completed invocation has no result".into())
					})?;
					let result = if aidash_domain::research::revocation::result_operation(&result)
						.is_some()
					{
						store
							.invocation_finish(run, token, &keys[offset], &result)
							.await?
					} else {
						result
					};
					outputs[offset] =
						Some(denial.map_or(result, |message| json!({"error":message})));
					continue;
				}
				"STARTED" => {}
				_ => {
					return Err(Error::Conflict(
						"batched invocation is not replay-safe".into(),
					));
				}
			}
			let tool = tool(call)?;
			let input = match denial {
				Some(message) => Err(message),
				None => match tool.admit(run, call.arguments.clone()).await {
					Ok(input) => Ok(input),
					Err(
						Error::Invalid(message)
						| Error::Domain(aidash_domain::Error::Invalid(message)),
					) => Err(message),
					Err(error) => return Err(error),
				},
			};
			dispatch.push((offset, tool, input));
		}
		let slots = self.environment.tool_slots();
		let admitted: &Run = run;
		let keys = &keys;
		let calls_in_flight = dispatch
			.into_iter()
			.map(|(offset, tool, input)| async move {
				let key = &keys[offset];
				// The slot covers every database connection of the call,
				// including the one that records its result.
				let _permit = match slots {
					Some(slots) => Some(slots.acquire().await?),
					None => None,
				};
				let output = match input {
					Err(message) => json!({"error":message}),
					Ok(input) => {
						Self::tool_delivery_result(
							store,
							admitted,
							token,
							key,
							tool.dispatch(admitted, input, key).await,
						)
						.await?
					}
				};
				// The stored result replaces a research result whose account
				// terms ended meanwhile (ADR 0058).
				let output = store
					.invocation_finish(admitted, token, key, &output)
					.await?;
				Ok((offset, output))
			});
		// Read-only calls may be dropped on cancellation; nothing is adopted and
		// the next step observes the cancelled Run. A newer input never
		// interrupts a batch, as it never interrupts a sequential call, so no
		// accepted input sequence can supersede this wait.
		let finished = tokio::select! {
			biased;
			interrupted = self.environment.wait_for_inference_interruption(admitted.id, i64::MAX) => {
				interrupted?;
				return Err(Error::Conflict("run cancelled during tool batch".into()));
			}
			finished = futures_util::future::join_all(calls_in_flight) => finished,
		};
		// Every call has finished or failed. Honor the longest delay requested by
		// any sibling; all their delivery outcomes have already been journaled.
		let mut retry: Option<(String, u32, Option<std::time::Duration>)> = None;
		let mut failure: Option<Error> = None;
		for result in finished {
			match result {
				Ok((offset, output)) => outputs[offset] = Some(output),
				Err(Error::ToolRetry {
					message,
					count,
					retry_after,
				}) => {
					if let Some((_, previous_count, previous_after)) = &mut retry {
						*previous_count = (*previous_count).max(count);
						*previous_after = (*previous_after).max(retry_after);
					} else {
						retry = Some((message, count, retry_after));
					}
				}
				Err(error) => {
					if failure.is_none() || !matches!(&error, Error::External(_)) {
						failure = Some(error);
					}
				}
			}
		}
		if failure
			.as_ref()
			.is_some_and(|error| !matches!(error, Error::External(_)))
		{
			return Err(failure.expect("present"));
		}
		if let Some((message, count, retry_after)) = retry {
			return Err(Error::ToolRetry {
				message,
				count,
				retry_after,
			});
		}
		if let Some(error) = failure {
			return Err(error);
		}
		let mut context = run.context.clone();
		capture_declared_coverage(&mut context, tools, false);
		for (offset, output) in outputs.into_iter().enumerate() {
			let call = &calls[cursor + offset];
			let output =
				output.ok_or_else(|| Error::Conflict("batched call has no result".into()))?;
			let contract = tool(call)?.contract();
			if contract
				.behavior
				.model_media_result(&call.arguments, &output)
				|| !matches!(
					FrameworkResult::decode(&contract, &output)?,
					FrameworkResult::Ordinary
				) {
				return Err(Error::Conflict(
					"batched call produced a continuation or model media".into(),
				));
			}
			let event = ContextEvent::tool(call.clone(), output.clone());
			record_message_read_for(&mut context.message_read_coverage, &contract, &event);
			let growth = context::tool_event_growth(&context, &event);
			context.push_invocation(event, &keys[offset]);
			let progress = run.state.tool_mut()?;
			progress.request_tokens = progress.request_tokens.saturating_add(growth);
			home.report(
				&format!("{}:tool", keys[offset]),
				"remote.tool.completed",
				json!({"run_id":run.id,"call":call,"result":context.history.last().expect("just appended tool result").durable_event()["result"]}),
			)
			.await?;
		}
		run.context = context;
		let progress = run.state.tool_mut()?;
		progress.workspace_read_plan = None;
		progress.skill_read_plan = None;
		progress.workspace_observation_plan = None;
		progress.advance(end);
		progress.batch_end = None;
		store.save_run(run, token, "run.tool_recorded").await?;
		Ok(true)
	}
	/// Record a non-accepted outcome of an Inference Attempt.
	async fn close_inference(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
		outcome: ProgressOutcome,
	) -> Result<()> {
		self.environment
			.store()
			.finish_inference(run, token, attempt, outcome)
			.await?;
		metrics::counter!("aidash_inference_interruptions_total", "reason" => outcome.reason())
			.increment(1);
		Ok(())
	}
	/// Record why an attempt ended without an Accepted Response. A failed write is
	/// only logged so the caller's error survives; a later attempt or terminal
	/// transition closes an attempt left pending.
	async fn interrupt_inference(
		&self,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
		reason: InterruptionReason,
	) {
		if let Err(error) = self
			.close_inference(run, token, attempt, ProgressOutcome::Interrupted(reason))
			.await
		{
			tracing::warn!(run_id = %run.id, %attempt, %error, "inference outcome was not recorded; a later attempt closes it");
		}
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
	/// The answer's Source citations and the cited references the Run's
	/// evidence store does not hold, when the Run binds an operation that
	/// declares `yields: Citations`. Declared behavior, not a tool name,
	/// selects verification.
	async fn citation_check(
		&self,
		run: &Run,
		tools: &Tools,
		text: &str,
	) -> Result<Option<(citations::Citations, Vec<citations::EvidenceRef>)>> {
		if !tools
			.values()
			.any(|tool| tool.contract().behavior.yields(Yield::Citations))
		{
			return Ok(None);
		}
		let found = citations::citations(text);
		if found.is_empty() {
			return Ok(None);
		}
		let unknown = self
			.environment
			.store()
			.unknown_evidence(run, &found.references)
			.await?;
		Ok(Some((found, unknown)))
	}
	/// Text published before the final answer is never corrected, so text
	/// citing evidence the Run never received is not published at all.
	async fn cites_invalid_evidence(&self, run: &Run, tools: &Tools, text: &str) -> Result<bool> {
		Ok(self
			.citation_check(run, tools, text)
			.await?
			.is_some_and(|(found, unknown)| {
				citations::verdict(&found, &unknown, true) != citations::Verdict::Accept
			}))
	}
	/// Record a provider-proven Context Overflow and re-enter Thinking with a
	/// smaller effective window, or pause once the bounded allowance is spent
	/// or the retried request did not shrink. Only recovery state is saved.
	/// An overflow after a calibrated estimate is an estimator defect: the Run
	/// is demoted to the byte estimate for the rest of its steps (ADR 0017),
	/// and later overflows are compared in bytes.
	async fn context_overflowed(
		&self,
		run: &mut Run,
		token: Uuid,
		request_tokens: usize,
		request_bytes: usize,
		policy: &context::policy::Effective,
	) -> Result<()> {
		let estimator = run.context.estimate.current().id();
		let mut recovery = run.context.recovery;
		let demoted = run.context.estimate.demote();
		let measured = if demoted {
			request_bytes
		} else {
			request_tokens
		};
		if !recovery.overflowed(
			measured as u64,
			policy.overflow_retries,
			policy.overflow_shrink_permille,
		) {
			metrics::counter!("aidash_context_overflow_total", "outcome" => "exhausted", "estimator" => estimator)
				.increment(1);
			// The pause saves no context. Save the demotion first, so a resumed
			// step never retries the calibrated estimate that overflowed.
			if demoted {
				self.environment.store().pin_estimate(run, token).await?;
			}
			return Err(Error::Context(
				context::recovery::Failure::OverflowRetriesExhausted,
			));
		}
		metrics::counter!("aidash_context_overflow_total", "outcome" => "retried", "estimator" => estimator).increment(1);
		run.context.recovery = recovery;
		self.environment
			.store()
			.save_run(run, token, "run.context_overflow")
			.await
	}
	/// Run one Summary Stage merge. Adoption saves the Run atomically and keeps
	/// it Thinking so the next step fits from durable state; every other
	/// outcome settles the attempt and leaves the saved projection unchanged.
	#[allow(clippy::too_many_arguments)]
	async fn summarize(
		&self,
		run: &mut Run,
		token: Uuid,
		guard: Option<&dyn ExecutionAuthority>,
		visibility: &mut dyn ExecutionVisibility,
		home: &dyn ExecutionHome,
		fitting: &aidash_application::context::Fitting<'_>,
		mut plan: aidash_application::context::SummaryPlan,
		tools: &Tools,
		staged: Option<&aidash_application::decision::StagedDecision>,
	) -> Result<()> {
		use aidash_application::context::{
			record_stage, record_tokens, summary_candidate, summary_request,
		};
		use context::recovery::{Attempt, Failure as ContextFailure, Outcome, Settlement, Stage};
		let store = self.environment.store();
		let unavailable = || {
			record_stage("summary", "unavailable");
			Error::Context(ContextFailure::SummaryUnavailable)
		};
		let (Some(summary), Some(max_tokens)) = (
			fitting.policy.summary.as_ref(),
			fitting.policy.summary_tokens(fitting.budget.window),
		) else {
			return Err(unavailable());
		};
		// Workspace records other than messages are filtered one by one, and a
		// summary rechecks only message sources: they stay verbatim instead.
		if fitting.native() {
			aidash_application::context::retain_summarizable_turns(&mut plan.absorbed, |event| {
				summarizable(event, tools)
			});
		} else {
			plan.absorbed
				.retain(|entry| summarizable(&entry.event, tools));
		}
		let (Some(first), Some(last)) = (plan.absorbed.first(), plan.absorbed.last()) else {
			return Err(Error::Context(ContextFailure::ContextUnreducible));
		};
		// Only the summarizer pinned in the Run's admission closure may run.
		let entry = match self
			.environment
			.catalog()
			.get_for_run(run, &summary.model.id, &summary.model.version)
			.await
		{
			Ok(entry) if entry.kind == "model" => entry,
			Ok(_) | Err(Error::NotFound(_) | Error::Invalid(_)) => return Err(unavailable()),
			Err(error) => return Err(error),
		};
		let summarizer = context::summary::SummaryProvider {
			model: summary.model.clone(),
			definition_digest: aidash_domain::registry::rules::digest(&serde_json::to_value(
				&entry,
			)?),
		};
		let model_cfg: ModelConfig = serde_json::from_value(entry.config)?;
		let summary_window = model_cfg.context_window;
		// The policy bound never exceeds what the pinned summarizer may emit.
		// The reservation charges that bound before the window cap, which a
		// remote Home can derive from the pinned definitions and verify.
		let reserved_output = summary.max_tokens.min(model_cfg.output_token_limit());
		let max_tokens = max_tokens.min(reserved_output);
		let provider = self
			.environment
			.provider(model_cfg.clone())
			.map_err(|_| unavailable())?;
		let request = summary_request(&plan, fitting.pinned, max_tokens);
		if request.ensure_fits(summary_window).is_err() {
			// The summarizer cannot read the absorbed history; never split it silently.
			record_stage("summary", "invalid");
			return Err(Error::Context(ContextFailure::SummaryInvalid));
		}
		// The candidate carries the previous summary's sources as well.
		let mut dependencies = summary_dependencies(&plan.absorbed, tools);
		if let Some(previous) = plan.pruned.execution_summary.as_deref() {
			dependencies
				.message_ids
				.extend(previous.dependencies.message_ids.iter().copied());
			dependencies
				.tool_call_ids
				.extend(previous.dependencies.tool_call_ids.iter().cloned());
		}
		let attempt = Attempt {
			id: Uuid::new_v4(),
			run_id: run.id,
			stage: Stage::Summary,
			policy_version: fitting.policy.version.into(),
			provider: format!(
				"{}@{}#{}",
				summary.model.id, summary.model.version, summarizer.definition_digest
			),
			source_from_seq: first.seq,
			source_through_seq: last.seq,
			base_revision: run.revision,
			observed_input_seq: run.observed_input_seq,
			before_tokens: plan.before_tokens as u64,
		};
		if let Err(error) = store
			.begin_compaction(run, token, &attempt, summary.call_budget)
			.await
		{
			if matches!(error, Error::Context(_)) {
				record_stage("summary", "unavailable");
			}
			return Err(error);
		}
		let settlement =
			|outcome: Outcome, reason: Option<&str>, after: Option<usize>| Settlement {
				outcome,
				reason: reason.map(str::to_owned),
				candidate_digest: None,
				after_tokens: after.map(|tokens| tokens as u64),
			};
		let attempt_id = attempt.id;
		let unauthorized = async |run: &Run, error: Error| -> Result<()> {
			store
				.settle_compaction(
					run,
					token,
					attempt_id,
					&settlement(Outcome::Unauthorized, None, None),
				)
				.await?;
			record_stage("summary", "unauthorized");
			Err(error)
		};
		// A source that is already revoked never charges the summarizer's
		// allowance.
		match summary_dependencies_current(self.environment, guard, home, run, &dependencies).await
		{
			Ok(true) => {}
			Ok(false) => return unauthorized(run, Error::Forbidden).await,
			Err(error) => return unauthorized(run, error).await,
		}
		let reservation = match guard {
			Some(guard) => match guard
				.reserve_summary(
					token,
					&summarizer,
					summary_window,
					reserved_output,
					&request,
				)
				.await
			{
				Ok(reservation) => reservation,
				Err(error) => {
					let outcome = if matches!(error, Error::Forbidden | Error::Unauthorized) {
						Outcome::Unauthorized
					} else {
						Outcome::Unavailable
					};
					store
						.settle_compaction(run, token, attempt.id, &settlement(outcome, None, None))
						.await?;
					record_stage("summary", outcome.label());
					return Err(error);
				}
			},
			None => None,
		};
		// A request to the summarizer cannot be recalled: every source the
		// candidate depends on must still be readable once the reservation,
		// which may itself wait on remote admission, holds. The request never
		// leaves this node otherwise, so its reservation is released.
		let current =
			summary_dependencies_current(self.environment, guard, home, run, &dependencies).await;
		if !matches!(current, Ok(true)) {
			let blocked = unauthorized(run, current.err().unwrap_or(Error::Forbidden)).await;
			if let Some(reservation) = reservation {
				reservation.abort().await?;
			}
			return blocked;
		}
		// Like inference, release visibility and authorization locks during I/O.
		if let Some(guard) = guard {
			guard.suspend().await?;
		}
		visibility.suspend().await?;
		// The compaction journal tracks adoption, while this Usage Record tracks
		// the physical provider call even when its result is never adopted.
		store
			.record_usage_dispatch(
				run,
				token,
				&UsageDispatch {
					attempt: attempt.id,
					lease: token,
					response_epoch: response_epoch(run.revision, run.step),
					model_id: summary.model.id.clone(),
					model_version: summary.model.version.clone(),
					projection_version: request.context.projection_version().number(),
					upstream_model: model_cfg.model_id.clone(),
					purpose: UsagePurpose::Summary,
					requested_route: model_cfg
						.provider_route
						.as_ref()
						.map(|route| route.tag.clone()),
					estimate: request.request_estimate(),
					shadow_estimates: diagnostic_estimates(
						&request,
						&model_cfg,
						request.context.projection_version(),
						run.step,
					),
					calibration_sample: false,
					cache_identity: request.cache_identity(),
				},
			)
			.await?;
		// Only cancellation interrupts a summary: no input sequence is newer
		// than `i64::MAX`, and new input does not change the summarized range.
		let result = tokio::select! {
			biased;
			interrupted = self.environment.wait_for_inference_interruption(run.id, i64::MAX) => match interrupted {
				Ok(_) => Err(Error::Conflict("run cancelled during context summary".into())),
				Err(error) => Err(error),
			},
			result = provider.infer(request, &aidash_application::ports::NoProgress) => result,
		};
		// Persist provider facts before resuming authorization or validating the
		// summary. Refusal, truncation and post-I/O revocation still incur work.
		let recorded = store
			.complete_usage_record(run, attempt.id, &usage_outcome(&result))
			.await;
		let resumed = visibility.resume().await;
		if let Some(reservation) = reservation
			&& let Some(response) = settled_usage(&result)
		{
			reservation.settle(response).await?;
		}
		let result = result.map_err(Error::settled);
		resumed?;
		recorded?;
		if let Some(guard) = guard {
			guard.resume().await?;
		}
		let response = match result {
			Ok(response) => response,
			// An overflowing, truncated or refused summary is never adopted, and
			// it does not recurse into another compaction.
			Err(error @ (Error::ContextOverflow | Error::Context(_))) => {
				let reason = match &error {
					Error::Context(failure) => failure.label(),
					_ => "context_overflow",
				};
				store
					.settle_compaction(
						run,
						token,
						attempt.id,
						&settlement(Outcome::Invalid, Some(reason), None),
					)
					.await?;
				record_stage("summary", "invalid");
				return Err(Error::Context(ContextFailure::SummaryInvalid));
			}
			Err(error) => {
				store
					.settle_compaction(
						run,
						token,
						attempt.id,
						&settlement(Outcome::Unavailable, None, None),
					)
					.await?;
				record_stage("summary", "unavailable");
				return Err(error);
			}
		};
		// Authority was released during I/O: recheck the Run's ordinary inference
		// authority and the exact summarizer when a guard holds them, and every
		// summarized source for every Run, before any candidate is adopted.
		let current = async {
			if let Some(guard) = guard {
				guard.inference().await?;
				guard.recheck_summary(&summarizer).await?;
				// The Decider's approval may have been withdrawn while the
				// summary ran; a staged decision is adopted only under it.
				if let Some(staged) = staged {
					guard.recheck_decider(&staged.evidence.decider).await?;
				}
			}
			summary_dependencies_current(self.environment, guard, home, run, &dependencies).await
		}
		.await;
		match current {
			Ok(true) => {}
			Ok(false) => return unauthorized(run, Error::Forbidden).await,
			Err(error) => return unauthorized(run, error).await,
		}
		match summary_candidate(
			&plan,
			&response.text,
			dependencies,
			summarizer,
			max_tokens,
			fitting,
		) {
			Ok((candidate, after)) => {
				let mut adopted = settlement(Outcome::Adopted, None, Some(after));
				adopted.candidate_digest = candidate
					.execution_summary
					.as_ref()
					.map(|summary| summary.digest.clone());
				let previous = std::mem::replace(&mut run.context, candidate);
				if let Err(error) = store
					.adopt_compaction(run, token, attempt.id, &adopted, staged)
					.await
				{
					run.context = previous;
					return Err(error);
				}
				record_stage("summary", "applied");
				record_tokens("summary", plan.before_tokens, after);
				Ok(())
			}
			Err(unadopted) => {
				store
					.settle_compaction(
						run,
						token,
						attempt.id,
						&settlement(
							unadopted.outcome,
							unadopted
								.rejection
								.as_ref()
								.map(|rejection| rejection.label()),
							unadopted.after_tokens,
						),
					)
					.await?;
				record_stage("summary", unadopted.outcome.label());
				Err(Error::Context(
					if unadopted.outcome == Outcome::Insufficient {
						ContextFailure::ContextUnreducible
					} else {
						ContextFailure::SummaryInvalid
					},
				))
			}
		}
	}
	/// A revoked source must not survive inside summary text. The restored
	/// originals are saved, but the step pauses for authority before Jev, the
	/// summarizer or the model can receive them. Later steps pause the same way
	/// until every source of the revoked summary is readable again.
	async fn pause_if_summary_revoked(
		&self,
		run: &mut Run,
		token: Uuid,
		guard: Option<&dyn ExecutionAuthority>,
		home: &dyn ExecutionHome,
		context: &mut Context,
	) -> Result<()> {
		if let Some(revoked) = &context.revoked_summary_dependencies {
			if !summary_dependencies_current(self.environment, guard, home, run, revoked).await? {
				return Err(Error::Forbidden);
			}
			context.revoked_summary_dependencies = None;
		}
		let Some(summary) = context.execution_summary.as_deref() else {
			return Ok(());
		};
		if summary_dependencies_current(self.environment, guard, home, run, &summary.dependencies)
			.await?
		{
			return Ok(());
		}
		let (from, through) = (summary.source.from_seq, summary.source.through_seq);
		let store = self.environment.store();
		let journal = store.context_journal(run.id, from, through).await?;
		aidash_application::context::restore_summarized(context, journal)?;
		aidash_application::context::record_stage("summary", "revoked");
		run.context = context.clone();
		store
			.save_run(run, token, "context.summary_revoked")
			.await?;
		Err(Error::Forbidden)
	}
	/// Revoked Research evidence never reaches a model provider again, not even
	/// inside an Execution Summary (ADR 0046). Once the Run's evidence is
	/// revoked, every research result in the projection becomes a placeholder,
	/// and an Execution Summary that absorbed any is discarded: its range is
	/// restored from the Context Journal and redacted the same way. Unlike a
	/// revoked summary source this never pauses, because revoked evidence is
	/// never readable again and the Run continues without research. The
	/// Context Journal retains immutable references whose owned bytes are removed.
	async fn apply_research_revocation(&self, run: &mut Run) -> Result<()> {
		use aidash_application::research::revocation;
		let store = self.environment.store();
		if !run.context.research_revoked
			&& (!revocation::may_carry_research(&run.context)
				|| !store.research_revoked(run).await?)
		{
			return Ok(());
		}
		revocation::revoke_projection(
			&mut run.context,
			&revocation::RunJournal { store, run: run.id },
		)
		.await
	}
	/// Provider results the Research account terms no longer keep never reach
	/// a model provider again (ADR 0058). Each research result built from one
	/// becomes the expired placeholder, keeping its call's arguments, and an
	/// Execution Summary that absorbed one is discarded and its range
	/// restored from the Context Journal, without pausing. A revoked Run's
	/// results are placeholders already.
	async fn apply_research_expiry(&self, run: &mut Run) -> Result<bool> {
		use aidash_application::research::{account_terms, revocation};
		if run.context.research_revoked || !revocation::may_carry_research(&run.context) {
			return Ok(false);
		}
		let store = self.environment.store();
		let expired = store.expired_research(run).await?;
		account_terms::expire_projection(
			&mut run.context,
			&expired,
			&revocation::RunJournal { store, run: run.id },
		)
		.await
	}
	pub async fn advance(
		&self,
		run: &mut Run,
		token: Uuid,
		visibility: &mut dyn ExecutionVisibility,
	) -> Result<()> {
		self.advance_step(run, token, visibility).await?;
		if run.phase().is_terminal() {
			self.environment.close_plugin_sessions(run.id).await;
		}
		Ok(())
	}
	/// Lists the remote servers of the Run's Agent Plugins once, at its first step, and pins the
	/// tools they offer (ADR 0073). A recovered Run that pinned them never lists again, and a
	/// crash before the save lists again.
	async fn pin_plugin_tools(
		&self,
		run: &mut Run,
		token: Uuid,
		guard: Option<&dyn ExecutionAuthority>,
		visibility: &mut dyn ExecutionVisibility,
	) -> Result<()> {
		let declares_plugins = run
			.context
			.binding_snapshot
			.as_deref()
			.is_some_and(|snapshot| !snapshot.agent_plugins.is_empty());
		if !declares_plugins || run.context.plugin_tools.is_some() {
			return Ok(());
		}
		// Like inference, release visibility and authorization locks during I/O.
		if let Some(guard) = guard {
			guard.suspend().await?;
		}
		visibility.suspend().await?;
		let pinned = self.environment.pin_plugin_tools(run).await;
		let resumed = visibility.resume().await;
		if let Some(guard) = guard {
			guard.resume().await?;
		}
		resumed?;
		let Some(pinned) = pinned? else {
			return Ok(());
		};
		run.context.plugin_tools = Some(pinned);
		self.environment.store().pin_plugin_tools(run, token).await
	}
	async fn advance_step(
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
			run.recovery.lease_recoveries = 0;
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
			run.recovery.lease_recoveries = 0;
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
		// Read before the retry state is cleared: this attempt may degrade an
		// unavailable workspace slot when it is the last one (ADR 0059).
		let degrade_on_outage = run.recovery.degrades_on_outage();
		run.recovery.retry = None;
		// The count survives lease-retaining checkpoints of this step; the save
		// that ends the step resets it (see `Run::worker_snapshot`).
		let lease_recoveries = run.recovery.lease_recoveries;
		if lease_recoveries > 0 {
			let data = json!({"run_id":run.id,"task_id":run.task_id,"phase":run.phase(),"cause":"expired worker lease","lease_recoveries":lease_recoveries});
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
		store.resolve_research(run).await?;
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
				self.record_start_content(run, &agent).await?;
				self.pin_plugin_tools(run, token, guard, visibility).await?;
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
				// Registration enforces the same pairing; never drop an opt-in
				// silently or send breakpoints to another route (ADR 0019).
				model_cfg.require_prompt_cache(agent.prompt_cache, agent.projection_version)?;
				let explicit_cache = agent.prompt_cache == PromptCache::Explicit;
				let input_format = aidash_domain::provider::InputFormat::for_model(&model_cfg, agent.projection_version);
				// A proven provider overflow shrinks the effective window for the Run.
				let window = run.context.recovery.effective_window(model_cfg.context_window);
				let policy = context::policy::Effective::of(agent.context_policy.as_ref());
				let output_limit = model_cfg.output_token_limit();
				let model = self.environment.provider(model_cfg.clone())?;
				let tools = self.environment.binding_resolver().tools(run).await?;
				self.apply_research_revocation(run).await?;
				self.apply_research_expiry(run).await?;
				let task = home.task().await?;
				if task.status == aidash_domain::TaskStatus::Completed {
					run.state = RunState::Completed(TerminalState {});
					store.save_run(run, token, "run.recovered").await?;
					return Ok(());
				}
				let inputs = store.run_inputs(run.id).await?;
				let projection = match agent.projection_version {
					ProjectionVersion::Legacy => context::RequestProjection::Legacy,
					ProjectionVersion::Ordered => {
						context::RequestProjection::Ordered(self.environment.cache_scope(run).await?)
					}
					ProjectionVersion::Native => {
						context::RequestProjection::Native(self.environment.cache_scope(run).await?)
					}
				};
				// Pin this step's Estimate Snapshot before any fitting relies on it
				// (ADR 0017). Only an opted-in, undemoted model needs calibration,
				// and only its snapshot is saved: a resumed step reuses it even if
				// the calibration data changed meanwhile. A tokenized model also
				// calibrates, for its byte quotas. Every other step is
				// conservative, so its Run keeps the context bytes it had.
				let estimate = match run.context.estimate.pinned(run.step) {
					Some(snapshot) if snapshot.usable() => snapshot,
					// The step pinned a Token Encoding this worker lacks: it
					// resumes on bytes, and the next step pins afresh.
					Some(_) => {
						let snapshot = EstimateSnapshot::conservative(run.step);
						run.context.estimate.snapshot = Some(snapshot);
						snapshot
					}
					None if model_cfg.request_estimator() != RequestEstimator::Bytes
						&& !run.context.estimate.demoted =>
					{
						let samples = store
							.calibration_samples(&model_cfg.model_id, agent.projection_version.number())
							.await?;
						let snapshot = EstimateSnapshot::choose(
							run.step,
							model_cfg.request_estimator(),
							false,
							&samples,
						);
						run.context.estimate.snapshot = Some(snapshot);
						store.pin_estimate(run, token).await?;
						// The saved projection contains immutable Research
						// references. Restore model-visible values before any
						// fitting or request construction uses the reloaded Run.
						store.resolve_research(run).await?;
						snapshot
					}
					None => EstimateSnapshot::conservative(run.step),
				};
				// Ordered and Native share the Stable Prefix rules: a Run-stable
				// snapshot quota and Retrieval Key reuse (ADR 0020). Only Ordered
				// keeps per-turn paragraphs out of `system`; Native keeps every
				// harness instruction in `system` and data in envelopes.
				let ordered = !projection.version().is_legacy();
				let turn_paragraphs = matches!(projection, context::RequestProjection::Ordered(_));
				let source_input_seq = inputs
					.last()
					.map_or(run.observed_input_seq, |input| input.seq);
				let source_boundary = format!("{}:{}:{}", run.step, source_input_seq, task.revision);
				let binding_digest = self
					.environment
					.catalog()
					.content_digest(&serde_json::to_string(&run.context.binding_snapshot)?);
				// Ordered Runs key their sources by provider retrieval keys and revisions.
				let source_plan = self.environment.context_source_plan(run)?;
				let cached_sources = if ordered || source_plan.iter().any(|source| source.refresh_each_step) {
					None
				} else {
					run.context
						.source_observation
						.as_ref()
						.map(|observation| observation.at(&source_boundary, &binding_digest))
						.transpose()?
						.flatten()
						.cloned()
				};
				if let Some(content) = &cached_sources {
					self.environment
						.recheck_source_observation(run, content)
						.await?;
				}
				let mut source_skill_context = String::new();
				let mut ordered_skill = OrderedSkill::Absent;
				let mut prepared_sources = vec![];
				// Exposure activation is independent of context providers. The native
				// provider receives prepared, attributed resident blocks in the frame.
				run.context.exposure.activate();
				let mut exposure = match deferred_exposure(run)? {
					Some((snapshot, budgets)) => {
						let catalog = exposure_catalog(self.environment, run, snapshot, &tools).await?;
						let selection = exposure::select(&budgets, &catalog, &run.context.exposure)?;
						if cached_sources.is_none() {
							prepared_sources =
								prepared_skill_sources(self.environment, run, snapshot, &catalog, &selection)
									.await?;
						}
						Some(selection)
					}
					None => None,
				};
				let source_frame = ContextSourceFrame {
					task: &task,
					entry: &entry,
					input_cutoff: source_input_seq,
					inputs: &[],
					budget: context::source::MAX_SOURCE_BYTES,
					key: None,
					degrade_on_outage,
					prepared: &prepared_sources,
				};
				let mut source_instruction_slots = std::collections::BTreeMap::<String, [usize; 2]>::new();
				let mut instructions = context::versioned_agent_instructions(agent.projection_version, "");
				for source in source_plan
					.iter()
					.filter(|source| source.phase == ContextSourcePhase::Instructions)
				{
					if source.target != ContextSourceTarget::Instructions {
						return Err(Error::Invalid(
							"instruction source targets request data".into(),
						));
					}
					let caches_text = matches!(
						source.cache,
						ContextSourceCache::Boundary | ContextSourceCache::Revision
					);
					let text = if ordered && source.cache == ContextSourceCache::Revision {
						let (text, observed) = self
							.ordered_instruction_source(run, token, &binding_digest, source, source_frame)
							.await?;
						ordered_skill = observed;
						text
					} else if let Some(cached) = cached_sources.as_ref().filter(|_| caches_text) {
						cached["skill_context"]
							.as_str()
							.unwrap_or_default()
							.to_owned()
					} else {
						read_planned_source(self.environment, run, token, source, source_frame)
							.await?
							.and_then(|value| value.as_str().map(str::to_owned))
							.unwrap_or_default()
					};
					if caches_text {
						source_skill_context.push_str(&text);
					}
					source_instruction_slots.insert(
						source.contribution_slot.clone(),
						[instructions.len(), instructions.len() + text.len()],
					);
					instructions.push_str(&text);
				}
				if let Some(selection) = &exposure {
					instructions.push_str(&selection.index);
				}
				instructions.push_str("\nAdditional user instructions:\n");
				instructions.push_str(&agent.instructions);
				let fixed_sources = collect_data_phase(
					self.environment,
					run,
					token,
					&source_plan,
					ContextSourcePhase::FixedData,
					source_frame,
				)
				.await?;
				let snapshot_sources = collect_data_phase(
					self.environment,
					run,
					token,
					&source_plan,
					ContextSourcePhase::SnapshotData,
					source_frame,
				)
				.await?;
				let mut context = run.context.clone();
				capture_declared_coverage(&mut context, &tools, false);
				copy_source_records(&run.context, &mut context);
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
					let record = home.read_record("message", &id.to_string()).await?;
					let cited = cited_versions(&record);
					let message: Message = serde_json::from_value(record)?;
					if message.workspace_id != run.workspace_id || message.content != input.content
					{
						return Err(Error::Conflict("run input message binding changed".into()));
					}
					if *reference_only {
						page_reads.push(id);
						let reference = json!({"seq":input.seq,"sender":input.sender,"record":{"kind":"message","id":id},"requires_workspace_read":true});
						run_messages.push(reference);
					} else {
						// A correction request must name the exact File version it
						// corrects, including after the message is summarized.
						let content = if cited.is_empty() {
							message.content
						} else {
							format!(
								"{}\n\nCited File versions: {}",
								message.content,
								cited.join("; ")
							)
						};
						run_messages.push(
							json!({"seq":input.seq,"sender":input.sender,"content":content}),
						);
					}
				}
				// URL intents live beside their inputs, never in their text
				// (ADR 0052): each run message names its own pages, and the
				// instruction below is rendered only into steps that carry any.
				let url_intents = self.environment.url_intents(run).await?;
				let task_pages: Vec<String> = url_intents
					.iter()
					.filter(|intent| intent.input_key.is_none())
					.flat_map(|intent| intent.pages.iter().cloned())
					.collect();
				// A run message folded into the run-message summary, or taken in
				// by an earlier media-intake turn that offered no tools, is on no
				// page again, so the pages it asked for are carried forward like
				// the Task's own, up to a Run-level bound.
				let carried: Vec<&[String]> = inputs
					.iter()
					.filter(|input| input.seq <= intake_seq)
					.flat_map(|input| {
						let key = input.idempotency_key.as_str();
						url_intents
							.iter()
							.filter(move |intent| intent.input_key.as_deref() == Some(key))
					})
					.map(|intent| intent.pages.as_slice())
					.collect();
				let mut current: Vec<&[String]> = Vec::new();
				for (position, (index, _)) in page.entries.iter().enumerate() {
					let key = inputs[*index].idempotency_key.as_str();
					let Some(intent) = url_intents
						.iter()
						.find(|intent| intent.input_key.as_deref() == Some(key))
					else {
						continue;
					};
					run_messages[position]["read_pages"] = json!(intent.pages);
					current.push(&intent.pages);
				}
				let requested_pages = aidash_domain::research::intent::requested_pages(
					&task_pages,
					&carried,
					&current,
				);
				let output = if run_message_catchup {
					output_limit.min(RUN_MESSAGE_SUMMARY_OUTPUT_LIMIT)
				} else {
					output_limit
				};
				// Ordered keeps per-turn paragraphs out of the cached system prompt.
				let mut turn_instructions = String::new();
				if run_message_catchup {
					(if turn_paragraphs { &mut turn_instructions } else { &mut instructions }).push_str(&format!(
						"\n\nRun-message catch-up: Treat the entries under run_messages as user task context. Read every required message record in this page before responding. Update the cumulative run_message_summary faithfully, preserving the user's goal, constraints, corrections, and unresolved requests in sequence order (newer corrections take precedence). Return only the concise updated summary, encoded in at most {run_message_limit} UTF-8 bytes. Do not answer the user, complete the task, publish text, or perform actions during catch-up.",
					));
				}
				if !run_message_catchup
					&& home.local()
					&& let Some(guard) = guard
					&& guard.auto_judging(run).await?
				{
					(if turn_paragraphs { &mut turn_instructions } else { &mut instructions }).push_str(AUTO_MODE_INSTRUCTION);
				}
				let mut pinned = json!({"identity":{"node_id":self.environment.node_id(),"agent_id":run.agent_id,"agent_version":run.agent_version},"task":task,"agent_state":{"phase":run.phase(),"step":run.step}});
                pinned.as_object_mut().expect("request snapshot").extend(snapshot_sources);
				if let Some(deferred_read) = &thinking.deferred_workspace_read {
					pinned["deferred_workspace_read"] = json!(deferred_read);
				}
				if let Some(deferred_read) = &thinking.deferred_skill_read {
					pinned["deferred_skill_read"] = json!(deferred_read);
				}
				if let Some(deferred_observation) = &thinking.deferred_workspace_observation {
					pinned["deferred_workspace_observation"] = json!(deferred_observation);
				}
				let withhold_answer_text = tools
					.values()
					.any(|tool| tool.contract().behavior.yields(Yield::Citations));
				let mut specifications = tools
					.iter()
					.filter(|(alias, _)| exposure.as_ref().is_none_or(|selection| selection.tools.contains(*alias)))
					.filter(|(_, tool)| !run_message_catchup || tool.contract().behavior.permits(ToolUseMode::MessageCatchUp))
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
					agent.projection_version,
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
				let media_intake = media.defer_human || media.defer_selected;
				if media_intake {
					specifications.clear();
					(if turn_paragraphs { &mut turn_instructions } else { &mut instructions }).push_str("\nMedia intake is continuing. For this interim turn, postpone required workspace reads and the cumulative run-message summary. Preserve the user goals, constraints, and corrections in these run messages and describe the media in this request as plain text. Do not call tools or complete the task; deferred media will be provided in the next request.");
				}
				// Catch-up and media intake send fewer tools; usage measures those sent.
				if (run_message_catchup || media_intake) && let Some(selection) = exposure.as_mut() {
					selection.restrict_tools(&specifications)?;
				}
				let context_window = window.saturating_sub(
					aidash_domain::provider::ModelRequest::content_parts_reservation(
						&media.parts,
						agent.projection_version,
						input_format,
					)?,
				);
				let private_context = if fixed_sources.is_empty() { Value::Null } else {Value::Object(fixed_sources.clone())};
				// Ordered per-turn paragraphs are request content the fixed
				// reservations must count, exactly as Legacy counts them in system.
				let turn_instructions = turn_instructions.trim_start().to_owned();
				let mut fixed_context = private_context.clone();
				if !turn_instructions.is_empty() {
					fixed_context["turn_instructions"] = json!(turn_instructions);
				}
				context::request_context_budget(
					window,
					output,
					&instructions,
					&specifications,
					&fixed_context,
					&projection,
					input_format,
					estimate,
				)
				.map_err(|_| unreducible(guard))?;
				let mut budget = context::RequestBudget {
					input_format,
					window: context_window,
					instructions: &instructions,
					tools: &specifications,
					max_output_tokens: output,
					projection: &projection,
					estimate,
				};
				let fit_window = |budget: &context::RequestBudget<'_>| {
					let minimum_request = budget
						.request(&Context::default(), &fixed_context)
						.estimated_total_tokens();
					let window = request_context_window(context_window, minimum_request);
					if force_read_compaction {
						force_workspace_read_compaction_window(window, minimum_request)
					} else {
						window
					}
				};
				budget.window = fit_window(&budget);
				// Run-fixed quotas derive from the conservative fit, so calibration
				// never moves the Stable Prefix or the semantic budget (ADR 0017).
				let mut conservative = budget.conservative();
				conservative.window = fit_window(&conservative);
				// Keep room for history and JSON message escaping. The final fitting
				// decision below measures the complete provider input, not this quota.
				// Snapshot quotas are bytes, so this step's token headroom converts
				// through its Estimate Snapshot.
				let available =
					estimate.bytes_for_tokens(budget.remaining(&Context::default(), &fixed_context));
				// Catch-up only summarizes; it never acts on a request. The URL
				// intents render beside the snapshot, so they are reserved before
				// it is bounded, like the run messages.
				let intent_payload = (!requested_pages.is_empty() && !run_message_catchup).then(|| {
					json!({
						"instruction": aidash_domain::research::intent::URL_INTENT_INSTRUCTION,
						"pages": requested_pages,
					})
				});
				let message_size = context::estimated_tokens(&json!(run_messages).to_string())
					+ intent_payload
						.as_ref()
						.map_or(0, |payload| context::estimated_tokens(&payload.to_string()));
				let volatile_budget = available.saturating_sub(message_size) / 4;
				let snapshot_fit = if ordered {
					// Stable fields use a Run-stable quota so every step renders the
					// same Stable Prefix bytes for the same task. It never exceeds
					// half the headroom left beside the fixed content, run messages
					// and volatile fields (the rest absorbs JSON string escaping and
					// history), so oversized fixed content truncates the snapshot,
					// or reports overflow, as Legacy does.
					let stable = conservative
						.remaining(&Context::default(), &fixed_context)
							.saturating_sub(message_size);
					let headroom = stable.saturating_sub(stable / 4) / 2;
					context::bound_ordered_snapshot(
						&mut pinned,
						context::ordered_stable_quota(window, output_limit).min(headroom),
						volatile_budget,
					)
				} else {
					context::bound_snapshot(&mut pinned, volatile_budget)
				};
				pinned.as_object_mut().expect("request snapshot").extend(fixed_sources);
				// The snapshot quota is a heuristic. Required IDs and other minimum
				// context may exceed it while the complete request still fits.
				// Keep the authorized run-directed input independent of the bounded
				// workspace preview. Admission has already capped its aggregate size.
				if !run_messages.is_empty() {
					pinned["run_messages"] = json!(run_messages);
				}
				if let Some(payload) = intent_payload {
					pinned["url_intents"] = payload;
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
				if !turn_instructions.is_empty() {
					pinned["turn_instructions"] = json!(turn_instructions);
				}
				if snapshot_fit.is_err()
					&& budget
						.request(&Context::default(), &pinned)
						.estimated_total_tokens()
						> budget.window
				{
					return Err(unreducible(guard));
				}
				// Legacy gives semantic memory half the remaining headroom. Ordered
				// keeps a Run-fixed budget (part of the Retrieval Key) so steps reuse
				// one read, but never more than that same headroom: pinned semantic
				// content cannot be compacted away later.
				let headroom = conservative.remaining(&Context::default(), &pinned) / 2;
				let semantic_budget = if ordered {
					context::sources::ordered_semantic_budget(window, output_limit).min(headroom)
				} else {
					headroom
				};
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
				let budgeted_frame = ContextSourceFrame {
					inputs: &semantic_inputs,
					budget: semantic_budget,
					prepared: &[],
					..source_frame
				};
				let retrieves = guard.is_some() || home.local();
				if ordered {
					let collected = self
						.ordered_data_sources(
							run,
							token,
							&source_plan,
							&binding_digest,
							retrieves,
							budgeted_frame,
						)
						.await?;
					copy_source_records(&run.context, &mut context);
					pinned
						.as_object_mut()
						.expect("request snapshot")
						.extend(collected.values.clone());
					if let Some(cause) = collected.degraded() {
						run.context = context.clone();
						degrade_context(store, run, token, cause).await?;
					} else if let Some(observation) = collected.observation(
						&pinned,
						&binding_digest,
						ordered_skill,
						context.source_observation.as_ref(),
					)? {
						context.source_observation = Some(observation);
						run.context = context.clone();
						store.observe_sources(run, token).await?;
					}
				} else {
					let values = if let Some(cached) = &cached_sources {
						cached_data_phase(&source_plan, ContextSourcePhase::BudgetedData, cached)
					} else if retrieves {
						collect_data_phase(
							self.environment,
							run,
							token,
							&source_plan,
							ContextSourcePhase::BudgetedData,
							budgeted_frame,
						)
						.await?
					} else {
						serde_json::Map::new()
					};
					copy_source_records(&run.context, &mut context);
					pinned
						.as_object_mut()
						.expect("request snapshot")
						.extend(values.clone());
					if let Some(cause) = values
						.values()
						.find_map(aidash_domain::semantic::degraded::degraded)
					{
						run.context = context.clone();
						degrade_context(store, run, token, cause).await?;
					} else if cached_sources.is_none() {
						let mut observed = serde_json::Map::from_iter([
							("memory".into(), pinned["memory"].clone()),
							("skill_context".into(), json!(source_skill_context)),
						]);
						observed.extend(values);
						context.source_observation = Some(context::sources::SourceObservation::new(
							source_boundary,
							binding_digest,
							Value::Object(observed),
						)?);
						run.context = context.clone();
						store.observe_sources(run, token).await?;
					}
				}
                self.pause_if_summary_revoked(run, token, guard, home.as_ref(), &mut context).await?;
                // History imported from a pre-journal Run may never have been
                // saved; journal it whole before pruning or a summary drops any.
                if run.context.journal.imported_through > run.context.journal.inferred_through {
                    store.journal_context(run, token).await?;
                }
                let fitting = aidash_application::context::Fitting { budget: &budget, pinned: &pinned, policy: &policy };
                // Every Run prunes only through its own compaction Decider; one
                // without a Decider may let the Summary Stage reduce it alone.
                let decider = if fitting.fits(&context) { None } else { self.environment.compaction_decider(run, token).await? };
                let compaction = compact_decided_execution(&mut context, run, token, self.environment.node_id(), decider.as_ref(), &fitting).await.map_err(|error| remote_budget(guard, error))?;
                let plan = match compaction {
                    aidash_application::context::Compaction::Fits => None,
                    // The decision saved the pruned context and released the
                    // lease, which ends the step; the next step rechecks the
                    // summary sources under its own lease.
                    aidash_application::context::Compaction::Saved => {
                        run.context = context;
                        return Ok(());
                    }
                    aidash_application::context::Compaction::NeedsSummary(plan) => Some(plan),
                };
                // Pruning may release authority for Jev I/O, and a local Run's
                // sources may change during it too; a source revoked meanwhile
                // must reach neither the summarizer nor the model.
                match plan {
                    None => self.pause_if_summary_revoked(run, token, guard, home.as_ref(), &mut context).await?,
                    Some(mut plan) => {
                        let staged = plan.staged.take();
                        let summarized = async {
                            self.pause_if_summary_revoked(run, token, guard, home.as_ref(), &mut context).await?;
                            Box::pin(self.summarize(run, token, guard, visibility, home.as_ref(), &fitting, *plan, &tools, staged.as_deref())).await
                        }.await;
                        // A pruning no summary completed changes nothing (ADR 0056).
                        if summarized.is_err() && let (Some(staged), Some(ports)) = (&staged, &decider) {
                            let gate = aidash_application::decision::DecisionGate { authority: ports.authority.as_ref(), journal: ports.journal.as_ref(), provider: ports.provider.as_ref() };
                            if let Err(error) = gate.reject_staged(staged).await {
                                tracing::warn!(run = %run.id, %error, "staged compaction decision was not recorded as rejected");
                            }
                        }
                        return summarized.map_err(|error| remote_budget(guard, error));
                    }
                }

				// Building the request awaited many reads, and evidence revoked
				// meanwhile must not reach the provider: decide once more right
				// before dispatch, and redo the step from the redacted projection.
				if !run.context.research_revoked
					&& aidash_application::research::revocation::may_carry_research(&context)
					&& store.research_revoked(run).await?
				{
					self.apply_research_revocation(run).await?;
					store.save_run(run, token, "run.research_revoked").await?;
					return Ok(());
				}
				if self.apply_research_expiry(run).await? {
					store.save_run(run, token, "run.research_expired").await?;
					return Ok(());
				}
				if let Some(guard) = guard {
					guard.inference().await?;
				}
				let mut request = budget.request(&context, &pinned);
				request.content_parts = media.parts;
				// Catch-up and media-deferral steps narrow or clear the tools,
				// which precede the cached prefix, so a cache write there would
				// never be read; they carry no breakpoints (ADR 0019).
				request.cache_breakpoints = explicit_cache
					&& !run_message_catchup
					&& !media.defer_human
					&& !media.defer_selected;
				request.disable_provider_transforms = !policy.is_legacy();
				request
					.ensure_fits(window)
					.map_err(|_| unreducible(guard))?;
				let source_delivery = fit_source_delivery(&context,&request,&source_instruction_slots,guard.is_some_and(|authority|authority.is_remote()) || run.context.binding_snapshot.as_ref().is_some_and(|snapshot|snapshot.remote))?;
                let source_request_digest=aidash_domain::registry::rules::digest(&serde_json::to_value(&request)?);
                let request_estimate = request.request_estimate();
				let request_tokens = request_estimate.tokens as usize;
				let request_bytes = request.byte_estimate().total();
				let media_inferred_seq_before_response = context.media_inferred_seq;
				let observed_input_seq_before_response = run.observed_input_seq;
				let reservation = if let Some(guard) = guard {
					guard
						.reserve_inference(token, window, output, &request)
						.await?
				} else {
					None
				};
				// Each provider call is a new Inference Attempt. Its Usage Record is
				// durable before provider I/O and is keyed apart from the lease; its
				// displayed progress shares the same identifier.
				let attempt = InferenceAttemptId::new();
				store
					.record_usage_dispatch(
						run,
						token,
						&UsageDispatch {
							attempt: attempt.0,
							lease: token,
							response_epoch: response_epoch(run.revision, run.step),
							model_id: agent.model.id.clone(),
							model_version: agent.model.version.clone(),
							projection_version: agent.projection_version.number(),
							upstream_model: model_cfg.model_id.clone(),
							purpose: UsagePurpose::Main,
							requested_route: model_cfg.provider_route.as_ref().map(|route| route.tag.clone()),
							shadow_estimates: diagnostic_estimates(&request, &model_cfg, agent.projection_version, run.step),
							estimate: request_estimate.clone(),
							// Media tokens are an allowance, not text, so they never calibrate.
							calibration_sample: request.content_parts.is_empty(),
							cache_identity: request.cache_identity(),
						},
					)
					.await?;
				// Race only inference, not replay-unsafe tools or durable transitions.
				// Release node-wide visibility and authorization row locks while the
				// provider waits; both boundaries are reacquired before accepting output.
				if let Some(guard) = guard {
					guard.suspend().await?;
				}
				visibility.suspend().await?;
				// Recording the attempt locks the Run row, which the authority scope
				// can hold until it is suspended.
				if let Err(error) = store.start_inference(run, token, attempt).await {
					visibility.resume().await?;
					return Err(error);
				}
				let started = tokio::time::Instant::now();
				let progress =
					progress::ProgressBuffer::withholding_text(started, withhold_answer_text);
				// Summary text may depend on a source revoked while the provider
				// runs, and stored progress is served live and on replay. Such a
				// Run's progress is held until the sources are rechecked after
				// the response; every other Run streams it as it arrives.
				let withhold_progress = context.execution_summary.is_some();
				let provider = async {
					let infer = async {
						let result = model.infer(request, &progress).await;
						progress.finish();
						result
					};
					if withhold_progress {
						infer.await
					} else {
						// The flusher stores the remainder before the outcome is recorded.
						tokio::join!(infer, progress.flush(store, run, token, attempt)).0
					}
				};
				let recheck = async {
					let Some(guard) = guard else {
						return std::future::pending().await;
					};
					loop {
						tokio::time::sleep(INFERENCE_RECHECK_INTERVAL).await;
						if let Err(error) = guard.recheck_inference().await {
							return error;
						}
					}
				};
				let inference = tokio::select! {
					biased;
					interrupted = self.environment.wait_for_inference_interruption(run.id, input_seq) => match interrupted {
						Ok(InferenceInterruption::Cancelled) => Inference::Interrupted(InterruptionReason::Cancelled, Error::Conflict("run cancelled during inference".into())),
						Ok(InferenceInterruption::Superseded) => Inference::Superseded,
						Err(error) => Inference::Interrupted(InterruptionReason::StreamError, error),
					},
					error = recheck => Inference::Interrupted(InterruptionReason::Revoked, error),
					result = provider => match result {
						Ok(response) => Inference::Response(Box::new(response)),
						Err(error @ Error::InferenceStalled) => Inference::Interrupted(InterruptionReason::Stall, error),
						Err(error) => Inference::Interrupted(InterruptionReason::StreamError, error),
					},
				};
				// A recheck dropped by another branch can stop after reacquiring the
				// authority scope; release it before outcome writes lock the Run row.
				if let Some(guard) = guard
					&& let Err(error) = guard.suspend().await
				{
					visibility.resume().await?;
					return Err(error);
				}
				let resumed = visibility.resume().await;
				// Cancellation, supersession, stalls and unreadable responses leave
				// usage unknown. A truncated or refused completion still reports its
				// billed usage, and a typed Context Overflow is a provider rejection.
				let outcome = match &inference {
					Inference::Response(response) => UsageOutcome::Completed(response.reported.clone()),
					Inference::Interrupted(_, error) => match error.terminal_usage() {
						Some(response) => UsageOutcome::Completed(response.reported.clone()),
						None if matches!(error, Error::ContextOverflow) => UsageOutcome::Rejected {
							class: "context_overflow".into(),
						},
						None if matches!(error, Error::ProviderRejected { .. }) => UsageOutcome::Rejected {
							class: "provider_rejected".into(),
						},
						// A plan call OpenAI refused, or one no Plan Connection could serve.
						None if matches!(error, Error::Plan(_)) => UsageOutcome::Rejected {
							class: "plan_rejected".into(),
						},
						None => UsageOutcome::Unknown,
					},
					Inference::Superseded => UsageOutcome::Unknown,
				};
				let recorded = store.complete_usage_record(run, attempt.0, &outcome).await;
				let result = match inference {
					Inference::Response(response) => *response,
					Inference::Superseded => {
						self.close_inference(run, token, attempt, ProgressOutcome::Discarded)
							.await?;
						resumed?;
						recorded?;
						if let Some(guard) = guard {
							guard.resume().await?;
						}
						// Like a stored response that predates new input: keep the
						// inference allowance and offer the same media, reads and
						// unconsumed deferrals to the next request.
						let pending = ToolCallState {
							media_inferred_seq_before_response,
							observed_input_seq_before_response,
							inferred_selected_media: selected_media,
							media_intake_through_seq: intake_seq,
							required_run_message_reads,
							..Default::default()
						};
						let next_pending = stale_media_pending(
							&mut context,
							&pending,
							&mut run.observed_input_seq,
						);
						run.context = context;
						run.state = RunState::Thinking(ThinkingState {
							force_workspace_read_compaction: thinking
								.force_workspace_read_compaction,
							deferred_workspace_read: thinking.deferred_workspace_read.clone(),
							deferred_skill_read: thinking.deferred_skill_read.clone(),
							deferred_workspace_observation: thinking
								.deferred_workspace_observation
								.clone(),
							..next_pending
						});
						store.save_run(run, token, "run.message_received").await?;
						return Ok(());
					}
					Inference::Interrupted(reason, error) => {
						self.interrupt_inference(run, token, attempt, reason).await;
						// A truncated or refused completion is billed even though its
						// output is never dispatched. A provider-proven Context
						// Overflow was rejected before any usage: release its whole
						// charge, so the reduced retry does not reserve on top of it.
						if let Some(reservation) = reservation {
							match error.terminal_usage() {
								Some(response) => reservation.settle(response).await?,
								None if matches!(error, Error::ContextOverflow) => {
									reservation.abort().await?
								}
								None => {}
							}
						}
						resumed?;
						if matches!(error, Error::ContextOverflow) {
							if let Some(guard) = guard {
								guard.resume().await?;
							}
							recorded?;
							return self
								.context_overflowed(run, token, request_tokens, request_bytes, &policy)
								.await;
						}
						// A failed record write never hides the inference error.
						return Err(error.settled());
					}
				};
				// Provider usage is billable even when authorization changed
				// or a transaction committed while its result was in flight.
				if let Some(reservation) = reservation
					&& let Err(error) = reservation.settle(&result).await
				{
					self.interrupt_inference(run, token, attempt, InterruptionReason::StreamError)
						.await;
					return Err(error);
				}
				if let Err(error) = resumed.and(recorded) {
					self.interrupt_inference(run, token, attempt, InterruptionReason::StreamError)
						.await;
					return Err(error);
				}
				if let Some(guard) = guard
					&& let Err(error) = guard.resume().await
				{
					// Authority changed while the provider ran; its output is never accepted.
					self.interrupt_inference(run, token, attempt, InterruptionReason::Revoked)
						.await;
					return Err(error);
				}
				// Automatic recall is fetched evidence until this response is adopted.
				// Refresh every staged bank under the resumed subject authority while
				// its lease remains held through the fenced Run save below. Use this
				// request's data even when degraded retrieval deliberately bypasses caching.
				if let Err(error) = self.environment.recheck_source_observation(run, &pinned).await
				{
					self.interrupt_inference(run, token, attempt, InterruptionReason::Revoked).await;
					return Err(error);
				}
				// Authority was released during inference, and a local Run's
				// sources may change meanwhile too: a summary source revoked
				// meanwhile discards the response before any of it is accepted,
				// published or dispatched.
				if let Err(error) = self
					.pause_if_summary_revoked(run, token, guard, home.as_ref(), &mut context)
					.await
				{
					self.interrupt_inference(run, token, attempt, InterruptionReason::Revoked)
						.await;
					return Err(error);
				}
				// Withheld progress reaches readers only once its sources hold.
				if withhold_progress {
					progress.flush(store, run, token, attempt).await;
				}
				// Count only tool content that survived compaction and was present
				// in a successful provider request, not every completed read.
				capture_declared_coverage(&mut context, &tools, true);
                for contribution in &source_delivery.contributions {
                    let receipt=context::source::accepted_delivery(contribution,source_request_digest.clone(),attempt.0)?;
                    context.source_deliveries.retain(|prior|prior.identity!=receipt.identity);
                    context.source_deliveries.push(receipt);
                }
				// Everything in this request reached an accepted inference, so the
				// Summary Stage may later absorb it without hiding unread content.
				context.journal.inferred_through = context.journal.head;
				context.recovery.inference_accepted();
				if let Some(seq) = media.through_seq {
					context.media_inferred_seq = context.media_inferred_seq.max(seq);
				}
				// More input than a calibrated or tokenized estimate predicted is
				// an estimator defect, such as a wrong Token Encoding: the Run
				// estimates in bytes from its next step (ADR 0017).
				if let Some(reported) = result.reported.input_tokens
					&& context.estimate.underestimated(&request_estimate, reported)
				{
					metrics::counter!("aidash_estimate_underestimated_total", "estimator" => estimate.id())
						.increment(1);
				}
				// Unreported counts are known only through the Usage Record. The
				// durable context keeps the shape older workers' strict decoders
				// accept, so a Run stays leasable across a rolling deployment.
				context.usage = Some(ContextUsage {
					input_tokens: result.input_tokens,
					output_tokens: result.output_tokens,
					context_window: window,
					compactions: context.compactions,
					exposure: exposure.map(|selection| selection.usage),
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
					home_approval: None,
					approval: None,
					batch_end: None,
					mcp: None,
				}));
				run.error = None;
				store.save_run(run, token, "model.completed").await?;
				metrics::histogram!("aidash_inference_accepted_seconds")
					.record(started.elapsed().as_secs_f64());
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
				// A preliminary MCP checkpoint has not admitted a wire send. It
				// must still yield to a correction instead of retrying stale admission.
				if run.state.tool()?.mcp.as_ref().is_none_or(|checkpoint| {
					checkpoint.sent.is_none() && checkpoint.input.is_none()
				}) && store
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
				let tools = self.environment.binding_resolver().tools(run).await?;
				let run_message_catchup = run.state.tool()?.run_message_catchup;
				if run_message_catchup {
					// A provider response cannot execute task tools during catch-up,
					// even if it returns calls that were not in the advertised tool set.
					result
						.tool_calls
						.retain(|call| tools.get(&call.name).is_some_and(|tool| tool.contract().behavior.permits(ToolUseMode::MessageCatchUp)));
				}
				let required_reads = run.state.tool()?.required_run_message_reads.clone();
				let mut context = run.context.clone();
				capture_declared_coverage(&mut context, &tools, false);
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
					&& !tools.get(&call.name).is_some_and(|tool| is_required_message_read_for(&tool.contract(), call, &required_reads, &context))
				{
					context.push(ContextEvent::RunMessageReadRequired {
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
					context.push(ContextEvent::RunMessageReadRequired {
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
						context.push(ContextEvent::RunMessageSummaryRequired {
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
						context.push(ContextEvent::RunMessageSummaryRequired {
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
					let summarized = |event: &ContextEvent| {
						declared_message_read_range(&tools, event)
							.is_some_and(|(id, _, _, _)| summarized_ids.contains(&id))
					};
					if agent.projection_version == ProjectionVersion::Native {
						// A Model Turn keeps every call it ran (ADR 0015).
						for entry in &mut context.history {
							if summarized(&entry.event)
								&& let ContextEvent::Tool { call, result } = &mut entry.event
							{
								*result = ContextEvent::elided_result(&call.id);
							}
						}
					} else {
						context.history.retain(|entry| !summarized(&entry.event));
					}
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
				// Text beside calls that cites evidence the Run never received is
				// not published, and its Model Turn records no text, so Context
				// inspection never shows what the gate withheld.
				let withheld = !result.tool_calls.is_empty()
					&& self.cites_invalid_evidence(run, &tools, &result.text).await?;
				if !run_message_catchup
					&& !result.tool_calls.is_empty()
					&& informed_response
					&& !withheld
				{
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
							// Not the final answer: gated like text beside calls.
							let withheld =
								self.cites_invalid_evidence(run, &tools, &result.text).await?;
							record_model_turn(
								run,
								agent.projection_version,
								if withheld { "" } else { &result.text },
								&result.tool_calls,
							)?;
							if !withheld {
								self.publish_model_text(home.as_ref(), guard, run, token, &result.text)
									.await?;
							}
							let failed = children.has_failed;
							if failed {
								if let Some(guard) = guard {
									guard.action("human.request", "run", run.id).await?;
								}
								let response_epoch = run.state.tool()?.response_epoch;
								let h=home.human_request(run,"INFORMATION_REQUEST","A subtask needs intervention. You can explicitly abandon failed, blocked or cancelled subtasks in their task details, providing a reason. Then answer this request to continue with the remaining results, or cancel this parent.",&format!("{}:{}:subtasks",run.id,response_epoch)).await?;
								run.state = RunState::Waiting(Box::new(WaitingState::Human {
									request_id: h.id,
									answer_by: None,
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
						// A final answer citing unknown evidence is corrected once, then
						// withheld.
						if let Some((found, unknown)) =
							self.citation_check(run, &tools, &result.text).await?
						{
							{
								// One correction per user input: a later turn's answer
								// gets its own correction.
								let corrected = run.context.events().any(|event| {
									matches!(event, ContextEvent::CitationCorrectionRequired { through_seq, .. } if *through_seq == run.observed_input_seq)
								});
								// A correction needs another inference step; at the step
								// limit the answer is withheld instead of failing the Run.
								let no_step_left = run.step.saturating_add(1) >= agent.max_steps;
								match citations::verdict(&found, &unknown, corrected || no_step_left) {
									citations::Verdict::Accept => {}
									citations::Verdict::Correct(invalid) => {
										let mut context = run.context.clone();
										context.push(ContextEvent::CitationCorrectionRequired {
											invalid,
											instruction: citations::CORRECTION_INSTRUCTION.into(),
											through_seq: run.observed_input_seq,
										});
										run.context = context;
										run.step += 1;
										run.state = RunState::Thinking(ThinkingState::default());
										store
											.save_run(run, token, "run.citation_correction_required")
											.await?;
										return Ok(());
									}
									citations::Verdict::Withhold => {
										result.text = citations::WITHHELD_ANSWER.into();
									}
								}
							}
						}
						// The Completion Gate: current Verification Evidence for the
						// exact mounted content, or no applicable Check (#112).
						if let completion::Gate::Saved = self
							.completion_gate(run, token, &agent, home.as_ref(), guard)
							.await?
						{
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
							run.context.completion.basis = None;
							run.state = RunState::Thinking(ThinkingState::default());

							store.save_run(run, token, "run.message_received").await?;
							return Ok(());
						}
						// The final answer is a Model Turn without calls. The citation
						// gate above already replaced a withheld answer.
						record_model_turn(
							run,
							agent.projection_version,
							&result.text,
							&result.tool_calls,
						)?;
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
								run.context.completion.basis = None;

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
				// The response is acted on from here: open its Model Turn before
				// the first of its Tool events.
				record_model_turn(
					run,
					agent.projection_version,
					if withheld { "" } else { &result.text },
					&result.tool_calls,
				)?;
				// Tool Batches run only in ordinary mode: no message catch-up, no
				// pending media selection and a response informed by required reads.
				let batch_end = match run.state.tool()?.batch_end {
					Some(end) => Some(end),
					None if !run_message_catchup
						&& informed_response
						&& pending_selected_media(run.state.tool()?).is_empty() =>
					{
						// A batch covers only calls the request's Exposure set
						// advertised, as checked below for a sequential call.
						let exposed = match deferred_exposure(run)? {
							Some((snapshot, budgets)) => {
								let catalog = exposure_catalog(self.environment, run, snapshot, &tools).await?;
								Some(exposure::select(&budgets, &catalog, &run.context.exposure)?.tools)
							}
							None => None,
						};
						let pending = run.state.tool()?;
						let ceiling = agent
							.tool_parallelism
							.min(self.environment.tool_slots().map_or(1, ToolSlots::ceiling));
						let mut context = run.context.clone();
						capture_declared_coverage(&mut context, &tools, false);
						let end = batch::plan_tool_batch(
							&context,
							&result.tool_calls,
							cursor,
							&tools,
							exposed.as_ref(),
							ceiling,
							batch::BatchBudget {
								request_tokens: pending.request_tokens,
								request_window: pending.request_window,
							},
						);
						(end > cursor).then_some(end)
					}
					None => None,
				};
				if let Some(end) = batch_end {
					let step = BatchStep {
						home: home.as_ref(),
						guard,
						tools: &tools,
						calls: &result.tool_calls,
						cursor,
						end,
					};
					if self.execute_batch(run, token, step).await? {
						return Ok(());
					}
				}
				let mut context = run.context.clone();
				capture_declared_coverage(&mut context, &tools, false);
				let mut call = result.tool_calls[cursor].clone();
				let Some(tool) = tools.get(&call.name) else {
					return self.tool_error(run, token, &call, cursor, format!("unavailable tool {}", call.name)).await;
				};
				if let Some((snapshot, budgets)) = deferred_exposure(run)? {
					// Every call is checked against the Exposure set its request
					// advertised; Load/Unload in this response apply to the next one.
					let catalog = exposure_catalog(self.environment, run, snapshot, &tools).await?;
					if !exposure::select(&budgets, &catalog, &run.context.exposure)?.tools.contains(&call.name) {
						let message = if exposure::select(&budgets, &catalog, &run.context.exposure.effective())?.tools.contains(&call.name) {
							format!("capability {} was loaded in this response; call it after the next model request", call.name)
						} else {
							format!("capability {} is not loaded; use capability_load", call.name)
						};
						return self.tool_error(run, token, &call, cursor, message).await;
					}
				}
				let contract = tool.contract();
				if !pending_selected_media(run.state.tool()?).is_empty() && !contract.behavior.permits(ToolUseMode::MediaPending) {
					return self.tool_error(run, token, &call, cursor, "selected model media must be inferred before this tool call; retry it after the next model response".into()).await;
				}
				// Retrieval must have current authority before any resource is fetched.
				if let Some(guard) = guard {
					match guard.tool(&call, &contract).await {
						Ok(()) => {},
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => return self.tool_error(run, token, &call, cursor, message).await,
						Err(error) => return Err(error),
					}
				}
				// A person who asked not to be asked is not asked by the model either.
				if contract.behavior.continuation == Continuation::Human
					&& home.local()
					&& let Some(guard) = guard
					&& guard.approval_mode(run).await? == Some(ApprovalMode::DontAsk)
				{
					return self.tool_error(run, token, &call, cursor, DONT_ASK_HUMAN_REQUEST.into()).await;
				}
				let mut prepared_result = None;
				if let Some(fitting) = contract.behavior.fitting_for(&call.arguments) {
					let (range, saved) = match tool_result_plan(run.state.tool()?, fitting, &call, run.step, cursor) {
						Ok(plan) => plan,
						Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => return self.tool_error(run, token, &call, cursor, message).await,
						Err(error) => return Err(error),
					};
					if let Some(output) = saved { prepared_result = Some(output); }
					else {
						let budget = WorkspaceReadFitBudget {
							offset: range.offset,
							requested: range.requested,
							request_tokens: run.state.tool()?.request_tokens,
							request_window: run.state.tool()?.request_window,
							remaining_calls: result.tool_calls.len().saturating_sub(cursor + 1),
						};
						let fitted = prepare_tool_result(home.as_ref(), tool.as_ref(), run, &context, &call, fitting, budget).await;
						match fitted {
							Ok(PreparedResult::Skip) => {},
							Ok(PreparedResult::NoEnvelopeRoom) => {
								if !run_message_catchup || !contract.behavior.catch_up { run.step += 1; }
								let (next, event) = defer_result(fitting, &call, pending_selected_media(run.state.tool()?));
								run.state = RunState::Thinking(next);
								store.save_run(run, token, event).await?;
								return Ok(());
							},
							Ok(PreparedResult::Fitted { call: bounded, result: output }) => {
								call = bounded;
								result.tool_calls[cursor] = call.clone();
								run.state.tool_mut()?.response = result.clone();
								let plan = ReadPlan { step: run.step, cursor, call: call.clone(), result: output.clone() };
								*result_plan_mut(run.state.tool_mut()?, fitting) = Some(plan);
								prepared_result = Some(output);
							},
							Err(Error::Invalid(message) | Error::Domain(aidash_domain::Error::Invalid(message))) => return self.tool_error(run, token, &call, cursor, message).await,
							Err(error) => return Err(error),
						}
					}
				}
				let call = &call;
				let response_epoch = run.state.tool()?.response_epoch;
				let call_key = format!("{}:{}:{}", run.id, response_epoch, cursor);
				// A call resumed after an approval raised inside it runs under its
				// own key, so the recorded `approval_required` result never replays.
				let key = match run.state.tool()?.approval {
					Some(approval) => format!("{call_key}:approval:{approval}"),
					None => call_key.clone(),
				};
				// Approval follows the admitted provider contract for every Agent. A
				// Read-only operation needs none, yet only the Tenant's own authority
				// can tell whether its policy forbids the call outright.
				if contract.behavior.tool_call_approval
					|| (contract.behavior.read_only_operation && home.local() && guard.is_some())
				{
					let target = match &contract.identity {
						ToolIdentity::Descriptor(reference) => ToolCallTarget::Registry(aidash_domain::registry::EntityRef {
							id: reference.id.clone(),
							version: reference.version.clone(),
						}),
						ToolIdentity::Registry(reference) => ToolCallTarget::Registry(reference.clone()),
						ToolIdentity::Builtin(_) => return Err(Error::Invalid("builtin cannot require external tool approval".into())),
						ToolIdentity::AgentPlugin(tool) => ToolCallTarget::AgentPlugin(tool.clone()),
					};
					if home.local() {
						let Some(guard) = guard else {
							return self
								.tool_error(run, token, call, cursor, "tool call approval is unavailable without scoped authority".into())
								.await;
						};
						let request = ToolCallRequest {
							key: call_key.clone(),
							tool: target.clone(),
							operation: INVOKE.into(),
							call: call.clone(),
							area: None,
							read_only: contract.behavior.read_only_operation,
						};
						let mut outcome = guard.tool_call_approval(run, request.clone()).await?;
						if let ToolCallApproval::Pending { approval_id, .. } = outcome {
							let pending = approval::PendingCall { call, tool: tool.as_ref(), contract: &contract, instructions: &agent.instructions };
							// Asked again, the approval answers as the Decider settled it.
							if self.judge_approval(run, token, guard, &pending, approval_id).await?.settled() {
								outcome = guard.tool_call_approval(run, request).await?;
							}
						}
						match outcome {
							ToolCallApproval::Approved { .. } => {}
							ToolCallApproval::Pending { approval_id, .. } => {
								run.state = RunState::Waiting(Box::new(WaitingState::Approval {
									approval: ApprovalRef::Capability { approval_id },
									resume: Box::new(run.state.tool()?.clone()),
								}));
								store
									.save_run(run, token, "run.waiting_for_tool_approval")
									.await?;
								return Ok(());
							}
							ToolCallApproval::Refused(refusal) => {
								return self
									.tool_error(run, token, call, cursor, refusal.message().into())
									.await;
							}
						}
					} else if let Some(decision) = run
						.state
						.tool()?
						.home_approval
						.as_ref()
						.filter(|d| d.key == call_key && d.call == *call)
					{
						if !decision.approved {
							return self
								.tool_error(
									run,
									token,
									call,
									cursor,
									"tool call approval was denied or expired".into(),
								)
								.await;
						}
					} else {
						if let Some(guard) = guard {
							guard.action("human.request", "run", run.id).await?;
						}
						// The person who can approve is at the Node the Run executes for.
						let prompt = format!(
							"Approve this exact external tool action once? Tool: {}; call: {}",
							target.label(),
							serde_json::to_string(call)?
						);
						let request = home
							.human_request(
								run,
								"APPROVAL_REQUIRED",
								&prompt,
								&format!("{call_key}:workbench-approval"),
							)
							.await?;
						run.state = RunState::Waiting(Box::new(WaitingState::Approval {
							approval: ApprovalRef::Home {
								request_id: request.id,
								key: call_key.clone(),
								call: call.clone(),
								expires_at: request.created_at + chrono::Duration::minutes(15),
							},
							resume: Box::new(run.state.tool()?.clone()),
						}));
						store
							.save_run(run, token, "run.waiting_for_tool_approval")
							.await?;
						return Ok(());
					}
				}
				let invocation = if tool.mcp_rounds() {
                    let Some(guard) = guard else { return self.tool_error(run, token, call, cursor, "MCP inputs require local authority".into()).await; };
                    let Some(output) = self.mcp_call(run, token, mcp::Call { tool: tool.as_ref(), call, key: &key, agent: &agent, guard, visibility }).await? else { return Ok(()); };
                    InvocationOutcome { status: "COMPLETED".into(), result: Some(output), uncertain_reason: None, retrying: false }
                } else {
                    store
					.invocation_start(
						run,
						token,
						&key,
						&call.name,
						&call.arguments,
						contract.replay_safe(),
					)
					.await?
                };
				if invocation.status == "UNCERTAIN" {
					if let Some(guard) = guard {
						guard.action("human.request", "run", run.id).await?;
					}
                    let reason = if invocation.uncertain_reason.as_deref() == Some("answer_lost") { "its answer was lost after the request was sent" } else { "the worker stopped" };
                    let prompt = format!("Tool {} may have completed before {reason}. Reconcile the external effect, then answer with a JSON object containing result. It will not be executed again. Invocation: {key}", call.name);
                    if home.local() {
                        store.reconciliation_request(run, token, &key, &prompt).await?;
                    } else {
                        let request = home.human_request(run, "CONFIRMATION", &prompt, &format!("{key}:reconcile")).await?;
                        run.state = RunState::Waiting(Box::new(WaitingState::Reconciliation { request_id: request.id, key: key.clone(), resume: Box::new(run.state.tool()?.clone()) }));
                    }
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
					Self::tool_delivery_result(store, run, token, &key, tool.invoke(run, call.arguments.clone(), &key).await).await?
				};
                // Revalidate Research before deriving any media selection. A completed
                // replay may now be a tombstone under expired account terms.
                let research_result = aidash_domain::research::revocation::result_operation(&output).is_some();
                let (output, unfinished_key) = if research_result {
                    (store.invocation_finish(run, token, &key, &output).await?, None)
                } else { (output, unfinished_key) };
				if let Some(selection) =
					contract.behavior.model_media_selection(&call.arguments, &output)
				{
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
							match check_model_media_headroom(headroom, parts, &model, agent.projection_version) {
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
                let output = if invocation.status != "COMPLETED" && !research_result {
                    store.invocation_finish(run, token, &key, &output).await?
                } else { output };
				// What a Tenant's HTTP or MCP tool returns is external data and never
				// raises an approval, which would repeat the external call.
				let framework = if contract.behavior.tenant_tool_call() {
					FrameworkResult::Ordinary
				} else {
					FrameworkResult::decode(&contract, &output)?
				};
				// An approval raised inside the call is not its result. The Run waits,
				// then makes the same call again under the approval's own key.
				if let FrameworkResult::Approval(approval_id) = framework {
					if let Some(guard) = guard {
						let pending = approval::PendingCall { call, tool: tool.as_ref(), contract: &contract, instructions: &agent.instructions };
						match self.judge_approval(run, token, guard, &pending, approval_id).await? {
							// The call runs again at once under the approval's own key.
							approval::Judged::Allowed => {
								run.state.tool_mut()?.approval = Some(approval_id);
								store.save_run(run, token, "run.approved_by_auto").await?;
								return Ok(());
							}
							approval::Judged::Blocked(reason) => {
								return self.tool_error(run, token, call, cursor, reason.refusal_message().into()).await;
							}
							approval::Judged::Undecided => {}
						}
					}
					let mut resume = run.state.tool()?.clone();
					resume.approval = Some(approval_id);
					run.state = RunState::Waiting(Box::new(WaitingState::Approval {
						approval: ApprovalRef::Capability { approval_id },
						resume: Box::new(resume),
					}));
					store.save_run(run, token, "run.waiting_for_approval").await?;
					return Ok(());
				}
				// Load/Unload results are staged with the same save that records
				// them and take effect with the next inference.
				if contract.behavior.exposure_update
					&& let Some(update) = output.get("exposure_update")
				{
					context
						.exposure
						.stage(serde_json::from_value::<exposure::ExposureUpdate>(update.clone())?);
				}
				let event = ContextEvent::tool(call.clone(), output.clone());
				record_message_read_for(&mut context.message_read_coverage, &contract, &event);
				let growth = context::tool_event_growth(&context, &event);
				context.push_invocation(event, &key);
				run.state.tool_mut()?.request_tokens =
					run.state.tool()?.request_tokens.saturating_add(growth);
				run.context = context.clone();
				let progress = run.state.tool_mut()?;
				progress.workspace_read_plan = None;
				progress.skill_read_plan = None;
				progress.workspace_observation_plan = None;
				progress.advance(cursor + 1);
				match framework {
					FrameworkResult::Human(request_id) => {
						// Only a model's question of a person on this Node has an
						// Answer Deadline; the person's settings are read now.
						let seconds = match guard {
							Some(guard) if home.local() => guard.answer_deadline(run).await?,
							_ => None,
						};
						run.step += 1;
						run.state = RunState::Waiting(Box::new(WaitingState::Human {
							request_id,
							answer_by: seconds
								.map(|seconds| chrono::Utc::now() + chrono::Duration::seconds(seconds)),
							resume: ResumeState::Thinking(ThinkingState::default()),
						}));
					}
					FrameworkResult::Wait(seconds) => {
						run.suspend(|resume| WaitingState::Timer {
							wake_at: chrono::Utc::now() + chrono::Duration::seconds(seconds),
							resume,
						})?
					}
					// An approval returned above, before the call was recorded.
					FrameworkResult::Ordinary | FrameworkResult::Approval(_) => {}
				}
				home.report(
					&format!("{key}:tool"),
					"remote.tool.completed",
					json!({"run_id":run.id,"call":call,"result":context.history.last().expect("just appended tool result").durable_event()["result"]}),
				)
				.await?;
				store.save_run(run, token, "run.tool_recorded").await?;
				Ok(())
			})
			.await?,
			RunState::Waiting(waiting) => {
				let mut waiting = *waiting;
				// A newer input ends a Completion Review without an answer: the
				// candidate under review is stale.
				let superseded = match waiting.request_id() {
					Some(id) if completion::review_superseded(store, run, id).await? => {
						completion::supersede_review(run, home.as_ref(), id).await?;
						true
					}
					_ => false,
				};
				if let Some(id) = waiting.request_id().filter(|_| !superseded) {
					if let Some(guard) = guard {
						guard.human_read(id).await?;
					}
                    let mut h = home.human_request_by_id(id).await?;
					if !home.local() && h.response.is_none() {
						// A remote scheduler polls Home; an unanswered continuation
						// remains waiting and advances the next polling deadline.
						store.save_run(run, token, "run.waiting").await?;
						return Ok(());
					}
					if h.response.is_none()
						&& let WaitingState::Human { answer_by: Some(answer_by), .. } = &waiting
					{
						if *answer_by > chrono::Utc::now() {
							// Woken by a clock that runs ahead of this one.
							store.save_run(run, token, "run.waiting").await?;
							return Ok(());
						}
						h = answer_at_deadline(home.as_ref(), id).await?;
					}
					let response = h.response.ok_or_else(||Error::Conflict("human request has not been answered".into()))?;
					match &mut waiting {
						WaitingState::Approval {
							approval:
								ApprovalRef::Home {
									key,
									call,
									expires_at,
									..
								},
							resume,
						} => {
							#[derive(serde::Deserialize)]
							struct ApprovalAnswer {
								approved: bool,
							}
							let decision =
								serde_json::from_value::<ApprovalAnswer>(response.clone()).ok();
							resume.home_approval = Some(HomeApproval {
								key: key.clone(),
								call: call.clone(),
								approved: decision.is_some_and(|d| d.approved)
									&& *expires_at > chrono::Utc::now(),
							})
						}
						WaitingState::Reconciliation { key, resume, .. } => {
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
                            let result = if resume.mcp.as_ref().is_some_and(|checkpoint| *key == format!("{}:mcp:round:{}", checkpoint.key, checkpoint.continuation_sends)) {
                                // A person's reconciliation is final tool data, even when
                                // it resembles the adapter's trusted protocol envelope.
                                serde_json::to_value(aidash_domain::agent_plugin::interaction::ToolRound::Complete(answer.result))?
                            } else { answer.result };
                            store.invocation_finish(run, token, key, &result).await?;
						}
						_ => {}
					}
					completion::answer_review(run, id, Some(&response));
					// MCP form data is disclosed to its server, not to the model.
					if h.kind != "MCP_ELICITATION"
						&& !matches!(waiting, WaitingState::Reconciliation { .. })
					{
						run.context.push(ContextEvent::Human {
							request: h.prompt,
							request_kind: h.kind,
							response: answer_for_model(response),
						});
					}
				}
				run.state = match waiting {
					WaitingState::Dependencies { resume, .. } => RunState::Ready(resume),
					WaitingState::Children { resume, .. } => RunState::Thinking(resume),
					WaitingState::Timer { resume, .. } | WaitingState::Human { resume, .. } => {
						resume.into_state()
					}
					WaitingState::Approval { resume, .. }
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

/// Answer a model's question on the system's behalf because its person's
/// Answer Deadline passed. A person who answered first wins, and so does a
/// system answer given before a crash.
async fn answer_at_deadline(home: &dyn ExecutionHome, request: Uuid) -> Result<HumanRequest> {
	match home
		.close_human_request(request, answer_deadline_response())
		.await
	{
		Err(Error::Conflict(_)) => home.human_request_by_id(request).await,
		closed => closed,
	}
}

/// Open the Model Turn of a response, with the text it may keep and its
/// calls, in a Native Run's Context Journal, once per response epoch however
/// often this boundary is re-entered. Legacy and Ordered Runs never record
/// one, so their requests keep their bytes.
fn record_model_turn(
	run: &mut Run,
	version: ProjectionVersion,
	text: &str,
	calls: &[aidash_domain::provider::ToolCall],
) -> Result<()> {
	if version != ProjectionVersion::Native {
		return Ok(());
	}
	let epoch = run.state.tool()?.response_epoch;
	let recorded = run.context.events().rev().any(
		|event| matches!(event, ContextEvent::Assistant { epoch: recorded, .. } if *recorded == epoch),
	);
	if recorded {
		return Ok(());
	}
	let event = ContextEvent::Assistant {
		epoch,
		text: text.to_owned(),
		calls: calls.iter().map(|call| call.id.clone()).collect(),
	};
	let growth = context::tool_event_growth(&run.context, &event);
	run.context.push(event);
	let pending = run.state.tool_mut()?;
	pending.request_tokens = pending.request_tokens.saturating_add(growth);
	Ok(())
}

/// Authority is rechecked at most this often while the provider call runs.
const INFERENCE_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

enum Inference {
	Response(Box<aidash_domain::provider::ModelResponse>),
	/// A newer Run input arrived; the attempt is discarded and replanned.
	Superseded,
	Interrupted(InterruptionReason, Error),
}
fn declared_message_read_range(
	tools: &Tools,
	event: &ContextEvent,
) -> Option<(Uuid, usize, usize, usize)> {
	let ContextEvent::Tool { call, .. } = event else {
		return None;
	};
	message_read_range_for(&tools.get(&call.name)?.contract(), event)
}
fn capture_declared_coverage(context: &mut Context, tools: &Tools, inference: bool) {
	let coverage = if inference {
		&mut context.message_inference_coverage
	} else {
		&mut context.message_read_coverage
	};
	for entry in &context.history {
		if let ContextEvent::Tool { call, .. } = &entry.event
			&& let Some(tool) = tools.get(&call.name)
		{
			record_message_read_for(coverage, &tool.contract(), &entry.event);
		}
	}
}

/// Source-specific adapters share fitting, prepared-state reuse and deferral.
async fn prepare_tool_result(
	home: &dyn ExecutionHome,
	tool: &dyn ExecutionTool,
	run: &Run,
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	fitting: ResultFitting,
	budget: WorkspaceReadFitBudget,
) -> Result<PreparedResult> {
	let (parameter, fitted) = match fitting {
		ResultFitting::WorkspaceRecord => {
			let (Some(kind), Some(id)) = (
				call.arguments["kind"].as_str(),
				call.arguments["id"].as_str(),
			) else {
				return Ok(PreparedResult::Skip);
			};
			let output = if let Some(workspace) = call.arguments["workspace_id"].as_str() {
				let workspace = workspace
					.parse()
					.map_err(|_| Error::Invalid("invalid workspace id".into()))?;
				home.read_subnode_record_chunk(workspace, kind, id, budget.offset, budget.requested)
					.await?
			} else {
				home.read_record_chunk(kind, id, budget.offset, budget.requested)
					.await?
			};
			let size = fit_tool_result(context, call, "max_chars", budget, 0, |chars| {
				workspace_read_result(&output, budget.requested, budget.offset, chars)
			});
			(
				"max_chars",
				size.map(|chars| {
					(
						chars,
						workspace_read_result(&output, budget.requested, budget.offset, chars),
					)
				}),
			)
		}
		ResultFitting::SkillText => {
			let output = tool.invoke(run, call.arguments.clone(), "").await?;
			let size = fit_tool_result(context, call, "max_chars", budget, 1, |chars| {
				skill_read_result(&output, chars)
			});
			(
				"max_chars",
				size.map(|chars| (chars, skill_read_result(&output, chars))),
			)
		}
		ResultFitting::SkillAsset => {
			let output = tool.invoke(run, call.arguments.clone(), "").await?;
			let returned = output["content"]
				.as_str()
				.map_or(0, |text| text.chars().count());
			let budget = WorkspaceReadFitBudget {
				requested: budget.requested.min(returned),
				..budget
			};
			let size = fit_tool_result(context, call, "max_chars", budget, 1, |chars| {
				skill_asset_result(&output, chars)
			});
			(
				"max_chars",
				size.map(|chars| (chars, skill_asset_result(&output, chars))),
			)
		}
		ResultFitting::CapabilityDescription => {
			let output = tool.invoke(run, call.arguments.clone(), "").await?;
			let event = ContextEvent::tool(call.clone(), output.clone());
			return Ok(if tool_event_fits(context, &event, budget) {
				PreparedResult::Fitted {
					call: call.clone(),
					result: output,
				}
			} else {
				PreparedResult::NoEnvelopeRoom
			});
		}
		ResultFitting::CapabilitySearch => {
			let output = tool.invoke(run, call.arguments.clone(), "").await?;
			return Ok(
				match fit_capability_search(context, call, &output, budget) {
					Some(result) => PreparedResult::Fitted {
						call: call.clone(),
						result,
					},
					None => PreparedResult::NoEnvelopeRoom,
				},
			);
		}
		ResultFitting::Observation => (
			"limit",
			home.observation_fitted(budget.offset, budget.requested, &|limit, output| {
				Ok(tool_result_fits(
					context,
					call,
					"limit",
					limit,
					output.clone(),
					budget,
				))
			})
			.await?,
		),
	};
	let Some((size, result)) = fitted else {
		return Ok(PreparedResult::NoEnvelopeRoom);
	};
	let mut bounded = call.clone();
	bounded.arguments[parameter] = json!(size);
	Ok(PreparedResult::Fitted {
		call: bounded,
		result,
	})
}
async fn resolve_model_input_media(
	environment: &dyn ExecutionEnvironment,
	projection: ProjectionVersion,
	guard: Option<&dyn ExecutionAuthority>,
	selections: &[ModelMedia],
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
	} else {
		guard
			.ok_or(Error::Forbidden)?
			.human_message_media(messages, model, projection)
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
		projection,
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
enum PreparedResult {
	Skip,
	NoEnvelopeRoom,
	Fitted {
		call: aidash_domain::provider::ToolCall,
		result: Value,
	},
}
/// Mandatory context that cannot fit pauses with a typed reason. Remote
/// executors keep reporting the established semantic context-budget pause.
fn unreducible(guard: Option<&dyn ExecutionAuthority>) -> Error {
	remote_budget(
		guard,
		Error::Context(context::recovery::Failure::ContextUnreducible),
	)
}
fn remote_budget(guard: Option<&dyn ExecutionAuthority>, error: Error) -> Error {
	match error {
		Error::Context(context::recovery::Failure::ContextUnreducible)
			if guard.is_some_and(|guard| guard.is_remote()) =>
		{
			Error::RemoteSemantic(Failure::ContextBudget)
		}
		error => error,
	}
}

/// The pinned snapshot and budgets of a `deferred@1` Run, read from its
/// Agent definition; `None` under `legacy@1` or without a Binding snapshot.
fn deferred_exposure(run: &Run) -> Result<Option<(&BindingSnapshot, exposure::DeferredBudgets)>> {
	let Some(snapshot) = run.context.binding_snapshot.as_deref() else {
		return Ok(None);
	};
	let agent = snapshot
		.definitions
		.iter()
		.find(|definition| definition.identity == snapshot.agent)
		.ok_or_else(|| Error::Invalid("snapshot lacks its Agent definition".into()))?;
	let policy =
		serde_json::from_value::<AgentBindings>(agent.definition.config.clone())?.exposure_policy();
	Ok(policy.budgets().map(|budgets| (snapshot, *budgets)))
}
/// Discoverable capabilities over the specifications dispatch advertises.
async fn exposure_catalog(
	environment: &dyn ExecutionEnvironment,
	run: &Run,
	snapshot: &BindingSnapshot,
	tools: &Tools,
) -> Result<Vec<exposure::Capability>> {
	let specs = tools
		.iter()
		.map(|(alias, tool)| (alias.clone(), tool.specification()))
		.collect();
	let direct = environment.direct_skills(run).await?;
	Ok(exposure::catalog(
		snapshot,
		&specs,
		&direct,
		run.context.plugin_tools.as_ref(),
	)?)
}
/// Resident blocks of the selected Skills: Registry bodies from the pinned
/// snapshot, direct bodies from the Run's pinned Skill files.
async fn prepared_skill_sources(
	environment: &dyn ExecutionEnvironment,
	run: &Run,
	snapshot: &BindingSnapshot,
	catalog: &[exposure::Capability],
	selection: &exposure::Selection,
) -> Result<Vec<PreparedSkillSource>> {
	let mut blocks = vec![];
	for alias in &selection.skills {
		let capability = catalog
			.iter()
			.find(|capability| &capability.alias == alias)
			.ok_or_else(|| {
				Error::Invalid(format!("selected Skill {alias} is not in the catalog"))
			})?;
		let body = match &capability.identity {
			exposure::CapabilityIdentity::Registry(_) => {
				Some(exposure::registry_skill_body(snapshot, capability)?)
			}
			exposure::CapabilityIdentity::DirectSkill { skill_id, .. } => {
				environment
					.direct_skill_body(run, *skill_id, &capability.digest)
					.await?
			}
			exposure::CapabilityIdentity::AgentPluginTool { .. } => {
				return Err(Error::Invalid(format!("selected {alias} is not a Skill")));
			}
		};
		let Some(body) = body else { continue };
		blocks.push(PreparedSkillSource {
			identity: capability.identity.clone(),
			digest: capability.digest.clone(),
			text: exposure::resident_block(capability, &body),
			preview: capability.description.clone(),
		});
	}
	Ok(blocks)
}
/// How this step obtained an Ordered Run's Skill context.
enum OrderedSkill {
	/// This request carries no Skill context.
	Absent,
	/// The cached entry is still current.
	Reused,
	/// Read now; cacheable once the Skill record has a revision.
	Retrieved(Option<context::sources::SkillObservation>),
}

/// An Ordered Run's semantic read for this step.
struct OrderedSemantic {
	values: serde_json::Map<String, Value>,
	boundary: String,
	retrieved: bool,
}
impl OrderedSemantic {
	/// The outage cause when this step retrieved Degraded context.
	fn degraded(&self) -> Option<aidash_domain::semantic::degraded::OutageCause> {
		self.retrieved
			.then(|| {
				self.values
					.values()
					.find_map(aidash_domain::semantic::degraded::degraded)
			})
			.flatten()
	}
	/// The observation to persist, or `None` when every cached entry was reused.
	/// Reused entries keep their original bytes and need no new journal.
	fn observation(
		&self,
		pinned: &Value,
		bindings: &str,
		skill: OrderedSkill,
		previous: Option<&context::sources::SourceObservation>,
	) -> Result<Option<context::sources::SourceObservation>> {
		let previous_skill = previous.and_then(|observation| observation.skill.clone());
		let (skill, skill_changed) = match skill {
			OrderedSkill::Absent => (None, previous_skill.is_some()),
			OrderedSkill::Reused => (previous_skill, false),
			OrderedSkill::Retrieved(skill) => (skill, true),
		};
		if !self.retrieved && !skill_changed {
			return Ok(None);
		}
		let content = if self.retrieved {
			{
				let mut content = self.values.clone();
				content.insert("memory".into(), pinned["memory"].clone());
				Value::Object(content)
			}
		} else {
			previous
				.ok_or_else(|| Error::Invalid("reused source observation is missing".into()))?
				.content
				.clone()
		};
		let mut observation = context::sources::SourceObservation::new(
			self.boundary.clone(),
			bindings.to_owned(),
			content,
		)?;
		observation.skill = skill;
		Ok(Some(observation))
	}
}

/// Record a step that ran with Degraded context. Only the step's first
/// degradation is saved, with `run.semantic_degraded`. A re-entry of the same
/// step saves nothing here: the degradation is already durable, and the
/// refreshed cause rides on the step's next save.
/// Recovery reloads the durable Run. Save an independently recovered
/// workspace before a later memory failure leaves this step.
async fn finish_source_read(
	store: &dyn ExecutionStore,
	run: &mut Run,
	token: Uuid,
	slot: &str,
	read: SemanticContextRead,
) -> Result<Option<Value>> {
	record_source_read(store, run, token, slot, &read).await?;
	if read.workspace_retrieved && run.recovery.degraded.outage {
		run.recovery.degraded.recovered();
		run.recovery.semantic_reason = None;
		run.recovery.retry = None;
		store.recover_sources(run, token).await?;
	}
	read.result
}

async fn degrade_context(
	store: &dyn ExecutionStore,
	run: &mut Run,
	token: Uuid,
	cause: aidash_domain::semantic::degraded::OutageCause,
) -> Result<()> {
	let new_step = run.recovery.degraded.record(run.step, cause);
	// The outage this step absorbed is over for retry purposes; a stale reason
	// must not make a later step's first failure look like its last retry.
	run.recovery.semantic_reason = None;
	if new_step {
		store.degrade_sources(run, token).await?;
	}
	Ok(())
}

/// Prune through the Run's compaction Decider, or without one let the Summary
/// Stage reduce the context alone. The context changes only when it fits.
async fn compact_decided_execution(
	context: &mut Context,
	run: &Run,
	token: Uuid,
	node: &str,
	decider: Option<&aidash_application::ports::decision::DecisionPorts<'_>>,
	fitting: &aidash_application::context::Fitting<'_>,
) -> Result<aidash_application::context::Compaction> {
	use aidash_application::context::{Compaction, Decision, compact_decided};
	use aidash_application::decision::{
		DecisionGate, Evaluation, compaction_disclosure, compaction_input_digest,
	};
	// Compaction replaces history only; the Exposure set is carried unchanged.
	let mut candidate = context.clone();
	context::observation::normalize_history(&mut candidate.history);
	let compaction = match decider {
		None => compact_decided(&mut candidate, None, fitting).await?,
		Some(ports) => {
			let boundary = aidash_domain::decision::Boundary {
				node: node.to_owned(),
				run: run.id,
				step: run.step,
				run_revision: run.revision,
				input_revision: run.observed_input_seq,
				worker: token,
				input_digest: compaction_input_digest(&candidate, fitting.budget, fitting.pinned)?,
				cycle: run.recovery.decision_cycle,
				retry: run.recovery.decider_retry,
			};
			let disclosure = compaction_disclosure(&candidate, fitting.pinned);
			let decision = Decision {
				gate: DecisionGate {
					authority: ports.authority.as_ref(),
					journal: ports.journal.as_ref(),
					provider: ports.provider.as_ref(),
				},
				evaluation: Evaluation {
					boundary: &boundary,
					decider: &ports.decider,
					definition: &ports.definition,
					disclosure: &disclosure,
					now: ports.journal.now(),
				},
			};
			compact_decided(&mut candidate, Some(&decision), fitting).await?
		}
	};
	if matches!(compaction, Compaction::Fits | Compaction::Saved) {
		*context = candidate;
	}
	Ok(compaction)
}

/// Local Runs recheck summary sources under worker authority without
/// recording reads. A remote executor asks its Home through the same
/// filtered record path that admitted the original reads.
async fn summary_dependencies_current(
	environment: &dyn ExecutionEnvironment,
	guard: Option<&dyn ExecutionAuthority>,
	home: &dyn ExecutionHome,
	run: &Run,
	dependencies: &context::summary::SummaryDependencies,
) -> Result<bool> {
	if !guard.is_some_and(|guard| guard.is_remote()) {
		return environment
			.summary_dependencies_current(run, dependencies)
			.await;
	}
	for id in &dependencies.message_ids {
		match home.read_record("message", &id.to_string()).await {
			Ok(_) => {}
			Err(Error::NotFound(_) | Error::Forbidden | Error::Unauthorized) => return Ok(false),
			Err(error) => return Err(error),
		}
	}
	Ok(true)
}
/// Observe the original request under frozen diagnostic inputs without changing execution.
fn diagnostic_estimates(
	request: &provider::ModelRequest,
	model: &ModelConfig,
	projection: ProjectionVersion,
	step: i32,
) -> Vec<ShadowEstimate> {
	let mut snapshots = model.diagnostic_snapshots(projection, step);
	if !snapshots.contains(&request.estimate) {
		snapshots.push(request.estimate);
	}
	snapshots
		.into_iter()
		.filter_map(|snapshot| {
			request
				.request_estimate_with_snapshot(&snapshot)
				.map(|estimate| ShadowEstimate { snapshot, estimate })
		})
		.collect()
}

fn usage_outcome(result: &Result<provider::ModelResponse>) -> UsageOutcome {
	match result {
		Ok(response) => UsageOutcome::Completed(response.reported.clone()),
		Err(error) => match error.terminal_usage() {
			Some(response) => UsageOutcome::Completed(response.reported.clone()),
			None if matches!(error, Error::ContextOverflow) => UsageOutcome::Rejected {
				class: "context_overflow".into(),
			},
			None if matches!(error, Error::ProviderRejected { .. }) => UsageOutcome::Rejected {
				class: "provider_rejected".into(),
			},
			None if matches!(error, Error::Plan(_)) => UsageOutcome::Rejected {
				class: "plan_rejected".into(),
			},
			None => UsageOutcome::Unknown,
		},
	}
}

/// Usage that settles an inference reservation: an accepted response, or a
/// truncated or refused one whose output is never dispatched.
fn settled_usage(result: &Result<provider::ModelResponse>) -> Option<&provider::ModelResponse> {
	match result {
		Ok(response) => Some(response),
		Err(error) => error.terminal_usage(),
	}
}
/// Whether the Summary Stage may absorb `event`. A message read is tracked
/// and rechecked by its target. Other workspace records, and observation
/// pages that preview tasks, artifacts, events and messages together, are
/// visible record by record, which a summary cannot recheck.
fn summarizable(event: &ContextEvent, tools: &Tools) -> bool {
	let ContextEvent::Tool { call, .. } = event else {
		return false;
	};
	let Some(contract) = tools.get(&call.name).map(|tool| tool.contract()) else {
		return true;
	};
	match contract.behavior.fitting {
		Some(ResultFitting::Observation) => false,
		Some(ResultFitting::WorkspaceRecord) => message_read_target(&contract, call).is_some(),
		_ => true,
	}
}
/// Sources whose current authority the summarized text depends on.
fn summary_dependencies(
	entries: &[context::HistoryEntry],
	tools: &Tools,
) -> context::summary::SummaryDependencies {
	let mut dependencies = context::summary::SummaryDependencies::default();
	for entry in entries {
		if let ContextEvent::Tool { call, .. } = &entry.event {
			dependencies.tool_call_ids.insert(call.id.clone());
			if let Some(id) = tools
				.get(&call.name)
				.and_then(|tool| message_read_target(&tool.contract(), call))
			{
				dependencies.message_ids.insert(id);
			}
		}
	}
	dependencies
}

mod approval;
mod completion;
mod mcp;
mod progress;
#[cfg(test)]
mod tests;

/// Persist proposals before a typed failure can pause execution. A retrieved
/// contribution earns delivery only after the actual inference is accepted.
async fn record_source_read(
	store: &dyn ExecutionStore,
	run: &mut Run,
	token: Uuid,
	slot: &str,
	read: &SemanticContextRead,
) -> Result<()> {
	run.context.source_contributions.retain(|contribution| {
		contribution.identity.item != slot
			&& !contribution.identity.item.starts_with(&format!("{slot}/"))
	});
	run.context
		.source_contributions
		.extend(read.contributions.clone());
	for selection in &read.selections {
		run.context.source_selections.retain(|prior| {
			!(prior.boundary.input_digest == selection.boundary.input_digest
				&& prior.decider == selection.decider)
		});
		run.context.source_selections.push(selection.clone());
	}
	if !read.selections.is_empty() {
		store.observe_sources(run, token).await?;
	}
	Ok(())
}
fn copy_source_records(from: &Context, to: &mut Context) {
	to.source_contributions = from.source_contributions.clone();
	to.source_selections = from.source_selections.clone();
}

/// Fit source envelopes against the final rendered request, after projection
/// clipping and compaction. Fetched/selected bytes absent here earn no receipt.
fn fit_source_delivery(
	context: &Context,
	request: &aidash_domain::provider::ModelRequest,
	instruction_slots: &std::collections::BTreeMap<String, [usize; 2]>,
	remote: bool,
) -> Result<aidash_application::context::source::FittedSources> {
	use context::source::*;

	let candidates = context
		.source_contributions
		.iter()
		.map(|contribution| SourceCandidate {
			identity: contribution.identity.clone(),
			content: contribution.content.clone(),
			preview: String::new(),
			provenance: contribution.provenance.clone(),
			dependencies: contribution.dependencies.clone(),
			completeness: contribution.completeness,
			detail: contribution.detail.clone(),
			mandatory: contribution.mandatory
				|| remote
					&& contribution.identity.item.split('/').next() == Some("semantic_memory"),
		})
		.collect();
	let mut fitted = aidash_application::context::source::fit_with(candidates, |items| {
		Ok(items.iter().all(|item| {
			let Some(original) = context
				.source_contributions
				.iter()
				.find(|original| original.identity == item.identity)
			else {
				return false;
			};
			let Some(placement) = &original.placement else {
				return false;
			};
			if placement.volatility.is_some() {
				return declared_present(request, original);
			}
			if placement.slot.ends_with("skills") {
				let Some([start, end]) = instruction_slots.get(&placement.slot).copied() else {
					return false;
				};
				let Some(text) = request.instructions.get(start..end) else {
					return false;
				};
				return text_placement(text, &placement.pointer) == item.content.as_str();
			}
			request_source_slot(request, &placement.slot)
				.and_then(|slot| slot.pointer(&placement.pointer).cloned())
				.as_ref() == Some(&item.content)
		}))
	})?;
	for contribution in &mut fitted.contributions {
		if let Some(original) = context
			.source_contributions
			.iter()
			.find(|original| original.identity == contribution.identity)
		{
			contribution.placement = original.placement.clone();
			contribution.fetched_digest = original.fetched_digest.clone();
		}
	}
	Ok(fitted)
}
fn text_placement<'a>(text: &'a str, pointer: &str) -> Option<&'a str> {
	let (start, end) = pointer.strip_prefix("/@text/")?.split_once('/')?;
	text.get(start.parse().ok()?..end.parse().ok()?)
}
fn request_source_slot(
	request: &aidash_domain::provider::ModelRequest,
	slot: &str,
) -> Option<Value> {
	use aidash_domain::provider::ModelContext;
	let key = if slot == "references" {
		"reference_documents"
	} else {
		slot
	};
	match &request.context {
		ModelContext::Sampling(_) => None,
		ModelContext::Legacy(context) => context.get("current")?.get(key).cloned(),
		ModelContext::Ordered(context) => {
			serde_json::from_str::<Value>(if key == "reference_documents" {
				&context.stable
			} else {
				&context.volatile
			})
			.ok()?
			.get(key)
			.cloned()
		}
		ModelContext::Native(context) => {
			if key == "reference_documents" {
				context
					.attachments
					.iter()
					.find(|envelope| {
						envelope.source
							== aidash_domain::projection::native::Source::ReferenceDocuments
					})
					.map(|envelope| envelope.content.clone())
			} else {
				serde_json::from_str::<Value>(&context.volatile)
					.ok()?
					.get(key)
					.cloned()
			}
		}
	}
}

/// Compact citation names for the Run-message summary: `path@digest-prefix
/// (File version <file_id>)`. Unknown or malformed entries are skipped.
pub(crate) fn cited_versions(record: &serde_json::Value) -> Vec<String> {
	record
		.get("citations")
		.and_then(serde_json::Value::as_array)
		.map(|citations| {
			citations
				.iter()
				.filter_map(|citation| {
					let path = citation.get("path")?.as_str()?;
					let digest = citation.get("digest")?.as_str()?;
					let file = citation.get("file_id")?.as_str()?;
					let prefix = &digest[..digest.len().min(12)];
					Some(format!("{path}@{prefix} (File version {file})"))
				})
				.collect()
		})
		.unwrap_or_default()
}
