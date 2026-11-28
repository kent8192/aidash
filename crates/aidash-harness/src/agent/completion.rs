//! Goal judgment after the working model stops without a tool call.

use super::*;
use aidash_application::ports::execution::completion::GOAL_JUDGE_PROMPT;
use aidash_domain::{
	goal::{Judgment, Verdict, VerdictRecord},
	model_call::{Dispatch, Outcome, Purpose},
	provider::{InputFormat, ModelContext, ModelRequest},
	registry::EntityRef,
};

impl Executor<'_> {
	pub(super) async fn restart_after_goal_edit(&self, run: &mut Run, token: Uuid) -> Result<()> {
		run.context.final_completion = None;
		run.context.goal_judging = Default::default();
		run.step += 1;
		run.state = RunState::Thinking(ThinkingState::default());
		self.environment
			.store()
			.save_run(run, token, "execution.goal_changed")
			.await
	}

	/// Outer `None` means this step saved a continuation. Inner `None`
	/// means the Task has no Goal and may complete.
	pub(super) async fn judge_goal(
		&self,
		run: &mut Run,
		token: Uuid,
		agent: &ExecutionAgent,
		home: &dyn ExecutionHome,
		visibility: &mut dyn ExecutionVisibility,
		answer: &str,
	) -> Result<Option<Option<Judgment>>> {
		let Some(goal) = home.task().await?.goal else {
			return Ok(Some(None));
		};
		let guard = self.environment.authority();
		let store = self.environment.store();
		let epoch = run.state.tool()?.response_epoch;
		let mut last_error = None;
		let mut record = run
			.context
			.goal_judging
			.last
			.as_deref()
			.filter(|last| {
				last.run_id == run.id && last.response_epoch == epoch && last.goal == goal
			})
			.cloned();
		if record.is_none() {
			for _ in 0..2 {
				match self
					.call_goal_judge(run, token, agent, visibility, &goal, answer)
					.await
				{
					Ok((judgment, model, cost_nanocredits)) => {
						record = Some(VerdictRecord {
							run_id: run.id,
							response_epoch: epoch,
							goal: goal.clone(),
							judgment,
							model,
							cost_nanocredits,
							judged_at: chrono::Utc::now(),
						});
						break;
					}
					Err(
						error @ (Error::BudgetExhausted { .. }
						| Error::Forbidden
						| Error::Unauthorized
						| Error::TransactionPending),
					) => return Err(error),
					Err(error) => last_error = Some(error),
				}
			}
		}
		if home.task().await?.goal.as_deref() != Some(goal.as_str()) {
			self.restart_after_goal_edit(run, token).await?;
			return Ok(None);
		}
		let prompt = if let Some(record) = record {
			let record = match home.record_goal_verdict(&record).await {
				Ok(record) => record,
				Err(_) if home.task().await?.goal.as_deref() != Some(&goal) => {
					return self
						.restart_after_goal_edit(run, token)
						.await
						.map(|()| None);
				}
				Err(error) => return Err(error),
			};
			if record.judgment.verdict != Verdict::NotYet {
				run.context.goal_judging.last = Some(Box::new(record.clone()));
				return Ok(Some(Some(record.judgment)));
			}
			let progress = &mut run.context.goal_judging;
			if progress
				.last
				.as_ref()
				.is_none_or(|last| last.response_epoch != epoch || last.goal != goal)
			{
				if progress.last.as_ref().is_some_and(|last| last.goal != goal) {
					progress.consecutive_not_yet = 0;
				}
				progress.consecutive_not_yet += 1;
			}
			let reason = record.judgment.reason.clone();
			progress.last = Some(Box::new(record));
			record_model_turn(run, answer, &[])?;
			let instruction = format!("The Goal is not yet met. Judge guidance: {reason}");
			run.context.push(ContextEvent::GoalFeedback { instruction });
			if run.context.goal_judging.consecutive_not_yet < 3 {
				run.step += 1;
				run.state = RunState::Thinking(ThinkingState::default());
				store.save_run(run, token, "execution.goal_not_yet").await?;
				return Ok(None);
			}
			format!(
				"The Goal judge returned not yet met three times without an intervening tool call. {reason}\nPlease clarify the Goal or advise how to proceed."
			)
		} else {
			let detail = last_error
				.map(|error| error.to_string())
				.unwrap_or_default();
			format!(
				"The Goal judge could not give a verdict after retrying. {detail}\nPlease advise how to proceed."
			)
		};
		if let Some(guard) = guard {
			guard.action("human.request", "run", run.id).await?;
		}
		let request = home
			.human_request(
				run,
				"INFORMATION_REQUEST",
				&prompt,
				&format!("{}:{epoch}:goal", run.id),
			)
			.await?;
		run.context.goal_judging.consecutive_not_yet = 0;
		run.step += 1;
		run.state = RunState::Waiting(Box::new(WaitingState::Human {
			request_id: request.id,
			answer_by: None,
			resume: ResumeState::Thinking(ThinkingState::default()),
		}));
		store.save_run(run, token, "execution.goal_waiting").await?;
		Ok(None)
	}

	#[allow(clippy::too_many_arguments)]
	async fn call_goal_judge(
		&self,
		run: &Run,
		token: Uuid,
		agent: &ExecutionAgent,
		visibility: &mut dyn ExecutionVisibility,
		goal: &str,
		answer: &str,
	) -> Result<(Judgment, EntityRef, Option<i64>)> {
		let guard = self.environment.authority();
		let store = self.environment.store();
		let definition = match self.environment.goal_judge_model(run).await? {
			Some(definition) => definition,
			None => {
				self.environment
					.catalog()
					.get_for_run(run, &agent.model.id, &agent.model.version)
					.await?
			}
		};
		let reference = EntityRef {
			id: definition.id,
			version: definition.version,
		};
		let mut config = ModelConfig::from_stored(definition.config)?;
		let output = config.output_token_limit().min(2048);
		config.max_output_tokens = Some(output);
		let transcript =
			run.context.goal_transcript.as_ref().ok_or_else(|| {
				Error::Invalid("The working model transcript is unavailable".into())
			})?;
		let request = ModelRequest {
			instructions: GOAL_JUDGE_PROMPT.into(),
			context: ModelContext::native_data(&json!({
				"goal": goal,
				"transcript": {"context": transcript, "answer": answer},
			})),
			max_output_tokens: output,
			disable_provider_transforms: true,
			input_format: InputFormat::for_model(&config),
			..Default::default()
		};
		let provider = self.environment.provider(config.clone())?;
		request.ensure_fits(config.context_window)?;
		if let Some(guard) = guard {
			guard.inference().await?;
		}
		let attempt = Uuid::new_v4();
		let dispatch = Dispatch::new(Purpose::GoalJudge, &request)?;
		let reservation = match guard {
			Some(guard) => {
				guard
					.reserve_inference(
						attempt,
						request
							.estimated_total_tokens()
							.saturating_sub(output as usize),
						output,
						&request,
					)
					.await?
			}
			None => None,
		};
		let dispatched = store
			.record_usage_dispatch(
				run,
				token,
				&UsageDispatch {
					attempt,
					lease: token,
					response_epoch: run.state.tool()?.response_epoch,
					model_id: reference.id.clone(),
					model_version: reference.version.clone(),
					projection_version: request.context.projection_version().number(),
					harness_contract: agent.harness_contract.number(),
					upstream_model: config.model_id,
					purpose: UsagePurpose::Auxiliary,
					model_call: Some(dispatch),
					requested_route: config
						.provider_route
						.as_ref()
						.map(|route| route.tag.clone()),
					estimate: request.request_estimate(),
					shadow_estimates: Vec::new(),
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
		if let Some(guard) = guard {
			guard.suspend().await?;
		}
		visibility.suspend().await?;
		let recheck = async {
			match guard {
				Some(guard) => wait_for_inference_refusal(guard).await,
				None => std::future::pending().await,
			}
		};
		let result = tokio::select! {
			biased;
			interrupted = self.environment.wait_for_inference_interruption(run.id, run.included_input_seq()) => {
				interrupted.and(Err(Error::TransactionPending))
			}
			error = recheck => Err(error),
			result = provider.infer(request, &aidash_application::ports::NoProgress) => result,
		};
		if let Some(guard) = guard {
			let suspended = guard.suspend().await;
			if let Err(error) = suspended {
				visibility.resume().await?;
				return Err(error);
			}
		}
		let parsed = result.as_ref().ok().and_then(|response| {
			response
				.tool_calls
				.is_empty()
				.then(|| Judgment::parse(&response.text))
				.and_then(|result| result.ok())
		});
		let outcome = if result.is_ok() && parsed.is_none() {
			Outcome::Failed
		} else {
			aidash_application::model_calls::outcome(&result)
		};
		let recorded = store
			.complete_model_call(run, attempt, &usage_outcome(&result), outcome)
			.await;
		let resumed = visibility.resume().await;
		if let Some(reservation) = reservation
			&& let Some(response) = settled_usage(&result)
		{
			reservation.settle(response).await?;
		}
		resumed?;
		recorded?;
		if let Some(guard) = guard {
			guard.resume().await?;
		}
		let response = result.map_err(Error::settled)?;
		let judgment =
			parsed.ok_or_else(|| Error::Invalid("The Goal judge returned invalid JSON".into()))?;
		Ok((
			judgment,
			reference,
			response.reported.cost.map(|cost| cost.nanocredits),
		))
	}
}
