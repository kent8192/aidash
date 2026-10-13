//! The Completion Gate (#112) between an Accepted Response without tool calls
//! and Task completion. It runs before `begin_final_completion`, so new input
//! is accepted while Checks run and simply makes the candidate stale.
use super::*;
use aidash_application::ports::execution::completion::{
	CheckRequest, CheckStart, CheckStatus, ContentState, MountedContent,
};
use aidash_domain::completion::{
	Candidate, CheckOutcome, CompletionBasis, Decision, PendingCheck, REPAIR_INSTRUCTION,
	RepairNote, ReviewReason, SHELL, decide, empty_content,
};

/// What the step does after the Completion Gate.
pub(super) enum Gate {
	/// Complete the Task with this basis.
	Proceed(CompletionBasis),
	/// The gate saved the Run in another state; the step ends.
	Saved,
}

/// How long a Run waits before it looks at a running Check or a busy Area again.
const CHECK_POLL_SECONDS: i64 = 2;
/// Completion Reviews are raised as intervention requests until the dedicated
/// request kind and its Completion Exception answer exist.
const REVIEW_KIND: &str = "INFORMATION_REQUEST";

impl Executor<'_> {
	/// Record the content a Run starts from, so `content_changed` Checks can
	/// tell whether the Run changed anything.
	pub(super) async fn record_start_content(
		&self,
		run: &mut Run,
		agent: &ExecutionAgent,
	) -> Result<()> {
		let Some(policy) = &agent.completion else {
			return Ok(());
		};
		if !policy.needs_start_content() || run.context.completion.start_content.is_some() {
			return Ok(());
		}
		// A node that cannot run Checks cannot see the content either; the
		// gate then pauses every candidate, so nothing is recorded.
		let Some(checks) = self.environment.completion_checks() else {
			return Ok(());
		};
		run.context.completion.start_content = Some(
			checks
				.content(run)
				.await?
				.map_or_else(empty_content, |content| content.digest),
		);
		Ok(())
	}

	pub(super) async fn completion_gate(
		&self,
		run: &mut Run,
		token: Uuid,
		agent: &ExecutionAgent,
		home: &dyn ExecutionHome,
		guard: Option<&dyn ExecutionAuthority>,
	) -> Result<Gate> {
		let Some(policy) = agent.completion.as_ref() else {
			return Ok(Gate::Proceed(CompletionBasis::NotRequired));
		};
		let now = chrono::Utc::now();
		let mut progress = run.context.completion.clone();
		progress.renew(run.state.tool()?.included_input_seq);
		// Without Checks the node cannot read the mounted content, so not even
		// `content_changed` can be decided: every candidate waits for a person.
		let Some(checks) = self.environment.completion_checks() else {
			let reason = ReviewReason::Unavailable {
				check: policy.checks()[0].name.clone(),
				detail: UNAVAILABLE.into(),
			};
			return self.review(run, token, home, guard, progress, reason).await;
		};
		// A started Check is harvested first. Its evidence belongs to the
		// content it ran on, whatever the candidate is now.
		if let Some(pending) = progress.pending.take() {
			let status = checks.status(run, pending.operation_id).await?;
			match status {
				CheckStatus::Running if !pending.overdue(now) => {
					progress.pending = Some(pending);
					return self
						.wait(run, token, progress, "run.completion_check_running")
						.await;
				}
				CheckStatus::Running => progress
					.record(pending.unavailable("the Check did not stop within its timeout", now)),
				CheckStatus::Finished {
					passed,
					exit_code,
					diagnostics,
				} => progress.record(pending.finish(passed, exit_code, &diagnostics, now)),
				CheckStatus::Unavailable(detail) => {
					progress.record(pending.unavailable(&detail, now))
				}
			}
		}
		let content = checks.content(run).await?;
		// The content is not settled while another operation runs in the Area.
		if content
			.as_ref()
			.is_some_and(|content| matches!(content.state, ContentState::Busy))
		{
			return self
				.wait(run, token, progress, "run.completion_content_busy")
				.await;
		}
		let capability = run
			.context
			.binding_snapshot
			.as_deref()
			.and_then(|snapshot| snapshot.operation(SHELL).ok())
			.map(|binding| binding.digest.clone());
		loop {
			let candidate = Candidate {
				content: content.as_ref().map(|content| content.digest.as_str()),
				capability: capability.as_deref(),
			};
			match decide(policy, &progress, &candidate) {
				Decision::Complete(basis) => {
					run.context.completion = progress;
					return Ok(Gate::Proceed(basis));
				}
				Decision::Start { check, subject } => {
					progress.attempts += 1;
					let attempt = progress.attempts;
					// `decide` starts a Check only for a Run with a Working Area.
					let start = match content.as_ref() {
						Some(MountedContent {
							revision,
							state: ContentState::Ready,
							..
						}) => {
							let request = CheckRequest {
								key: subject.key(run.id, attempt),
								command: &check.command,
								timeout_seconds: check.timeout_seconds,
								revision: *revision,
							};
							checks.start(run, &request).await?
						}
						Some(MountedContent {
							state: ContentState::Unavailable(detail),
							..
						}) => CheckStart::Unavailable(detail.clone()),
						_ => CheckStart::Unavailable(UNAVAILABLE.into()),
					};
					match start {
						CheckStart::Started {
							operation_id,
							timeout_seconds,
						} => {
							progress.pending = Some(PendingCheck {
								subject,
								operation_id,
								attempt,
								timeout_seconds,
								started_at: now,
							});
							return self
								.wait(run, token, progress, "run.completion_check_started")
								.await;
						}
						CheckStart::Stale => {
							return self
								.wait(run, token, progress, "run.completion_content_changed")
								.await;
						}
						CheckStart::Unavailable(detail) => progress.record(subject.refused(
							attempt,
							CheckOutcome::Unavailable,
							&detail,
							now,
						)),
						CheckStart::Denied(detail) => progress.record(subject.refused(
							attempt,
							CheckOutcome::Denied,
							&detail,
							now,
						)),
					}
				}
				// Repair needs another inference step; at the step limit the
				// candidate goes to a person instead of failing the Run.
				Decision::Repair(evidence) if run.step.saturating_add(1) < agent.max_steps => {
					progress.repairs += 1;
					let through_seq = run.state.tool()?.included_input_seq;
					let mut context = run.context.clone();
					context.completion = progress;
					context.push(ContextEvent::CompletionRepairRequired {
						checks: vec![RepairNote::from(&evidence)],
						instruction: REPAIR_INSTRUCTION.into(),
						through_seq,
					});
					run.context = context;
					run.step += 1;
					run.state = RunState::Thinking(ThinkingState::default());
					self.environment
						.store()
						.save_run(run, token, "run.completion_repair_required")
						.await?;
					return Ok(Gate::Saved);
				}
				Decision::Repair(evidence) => {
					let reason = ReviewReason::RepairExhausted {
						check: evidence.subject.check,
					};
					return self.review(run, token, home, guard, progress, reason).await;
				}
				Decision::Review(reason) => {
					return self.review(run, token, home, guard, progress, reason).await;
				}
			}
		}
	}

	/// Look again shortly, resuming the same candidate.
	async fn wait(
		&self,
		run: &mut Run,
		token: Uuid,
		progress: aidash_domain::completion::CompletionProgress,
		event: &str,
	) -> Result<Gate> {
		run.context.completion = progress;
		run.suspend(|resume| WaitingState::Timer {
			wake_at: chrono::Utc::now() + chrono::Duration::seconds(CHECK_POLL_SECONDS),
			resume,
		})?;
		self.environment.store().save_run(run, token, event).await?;
		Ok(Gate::Saved)
	}

	/// Pause for a Completion Review. Its answer resumes the same candidate.
	async fn review(
		&self,
		run: &mut Run,
		token: Uuid,
		home: &dyn ExecutionHome,
		guard: Option<&dyn ExecutionAuthority>,
		mut progress: aidash_domain::completion::CompletionProgress,
		reason: ReviewReason,
	) -> Result<Gate> {
		if let Some(guard) = guard {
			guard.action("human.request", "run", run.id).await?;
		}
		progress.reviews += 1;
		let key = format!(
			"{}:{}:completion-review:{}",
			run.id,
			run.state.tool()?.response_epoch,
			progress.reviews
		);
		let request = home
			.human_request(run, REVIEW_KIND, &reason.prompt(), &key)
			.await?;
		progress.review = Some(request.id);
		run.context.completion = progress;
		run.suspend(|resume| WaitingState::Human {
			request_id: request.id,
			resume,
		})?;
		self.environment
			.store()
			.save_run(run, token, "run.completion_review_required")
			.await?;
		Ok(Gate::Saved)
	}
}

/// Whether the Completion Review the Run waits on ends without an answer:
/// a newer accepted input supersedes the candidate under review.
pub(super) async fn review_superseded(
	store: &dyn ExecutionStore,
	run: &Run,
	request: Uuid,
) -> Result<bool> {
	if run.context.completion.review != Some(request) {
		return Ok(false);
	}
	let included = run.included_input_seq();
	Ok(store
		.run_inputs(run.id)
		.await?
		.last()
		.is_some_and(|input| input.seq > included))
}

/// Apply a Completion Review answer: `retry`, as text or `{"retry": true}`,
/// runs the refused Checks again.
pub(super) fn answer_review(run: &mut Run, request: Uuid, response: Option<&Value>) {
	let progress = &mut run.context.completion;
	if progress.review != Some(request) {
		return;
	}
	progress.review = None;
	if response.is_some_and(|response| {
		response["retry"] == true
			|| response
				.as_str()
				.is_some_and(|text| text.trim().eq_ignore_ascii_case("retry"))
	}) {
		progress.retry();
	}
}

const UNAVAILABLE: &str = "Completion Checks cannot run on this node";
