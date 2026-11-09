//! Durable worker recovery. Retrying an outbox never repeats a failed tool.
use aidash_application::{
	Result, authorization::refusal_note, ports::ExecutionRecoveryStore, recovery::ExecutionFailure,
};
use aidash_domain::semantic::{Failure as SemanticFailure, OUTAGE_RETRIES, PENDING_REPLAY_SECONDS};
use aidash_domain::{FailureTarget, RetryState, Run, RunControl, RunPhase, RunState, WaitingState};
use chrono::{DateTime, Utc};
use uuid::Uuid;

fn plain_tool_name(name: &str) -> bool {
	(1..=64).contains(&name.len())
		&& name
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// The stage an unnamed authority refusal is reported under: the tool being called, else the
/// Run's phase.
fn stage(run: &Run) -> String {
	match run.phase() {
		RunPhase::ToolCall => run
			.state
			.tool()
			.ok()
			.and_then(|state| state.response.tool_calls.get(state.cursor))
			// The name comes from the model's response and reaches the Run's `error`, so only a
			// plain tool name is shown.
			.filter(|call| plain_tool_name(&call.name))
			.map_or_else(|| "tool call".into(), |call| call.name.clone()),
		RunPhase::Ready => "worker start".into(),
		RunPhase::Thinking => "model call".into(),
		RunPhase::Waiting => "resume".into(),
		RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled => "run".into(),
	}
}

/// Returns whether a generic worker-retry metric should be incremented.
/// Every write is fenced by the original worker token in the repository.
pub async fn recover(
	store: &dyn ExecutionRecoveryStore,
	token: Uuid,
	failure: ExecutionFailure,
	now: DateTime<Utc>,
) -> Result<bool> {
	let Some(mut current) = store.leased_run(token).await? else {
		return Ok(false);
	};
	let attempts = current
		.recovery
		.retry
		.as_ref()
		.map_or(0, |retry| retry.count)
		+ 1;
	if let ExecutionFailure::Semantic(reason) = failure {
		if reason == SemanticFailure::Stale && attempts <= OUTAGE_RETRIES {
			// The Home refused a reused retrieval whose corpus changed (ADR
			// 0020). Drop the cached Source observation so the retry retrieves
			// once more, under a READY operation of its own step.
			current.context.source_observation = None;
			current.recovery.semantic_reason = Some(reason);
			current.recovery.retry = Some(RetryState {
				count: attempts,
				at: now + chrono::Duration::seconds(1),
			});
			current.error = Some(reason.to_string());
			store.save(&current, token, "run.semantic_retrying").await?;
		} else if reason == SemanticFailure::Pending
			|| (reason.transient() && attempts <= OUTAGE_RETRIES)
		{
			let delay = if reason == SemanticFailure::Pending {
				// Each replay repeats the whole semantic-context step, including
				// its charged workspace embedding, so only the first check of a
				// live memory attempt comes after a second; later ones wait
				// PENDING_REPLAY_SECONDS until the attempt answers or expires.
				if current.recovery.retry.is_some()
					&& current.recovery.semantic_reason == Some(SemanticFailure::Pending)
				{
					PENDING_REPLAY_SECONDS
				} else {
					1
				}
			} else {
				2_i64.pow(attempts)
			};
			current.recovery.semantic_reason = Some(reason);
			current.recovery.retry = Some(RetryState {
				count: if reason == SemanticFailure::Pending {
					attempts - 1
				} else {
					attempts
				},
				at: now + chrono::Duration::seconds(delay),
			});
			current.error = Some(reason.to_string());
			store.save(&current, token, "run.semantic_retrying").await?;
		} else {
			let reason = if reason.transient() || reason == SemanticFailure::Stale {
				SemanticFailure::RetriesExhausted
			} else {
				reason
			};
			current.recovery.semantic_reason = Some(reason);
			store.pause_semantic(&current, token, reason).await?;
		}
	} else if matches!(failure, ExecutionFailure::DeciderOutage) {
		// Every Decider Route failed. Retry after 2, 4, 8, 16 and 32 s, each
		// time as a new decision, then pause until a person resumes (ADR 0066).
		let retries = current.recovery.decider_retry + 1;
		if retries <= OUTAGE_RETRIES {
			current.recovery.decider_retry = retries;
			current.recovery.retry = Some(RetryState {
				count: retries,
				at: now + chrono::Duration::seconds(2_i64.pow(retries)),
			});
			current.error = Some(failure.message());
			store.save(&current, token, "run.decider_retrying").await?;
		} else {
			let reason = aidash_domain::context::recovery::Failure::DeciderUnavailable;
			current.recovery.context_reason = Some(reason);
			store.pause_context(&current, token, reason).await?;
		}
	} else if let ExecutionFailure::NodeUnreachable { retry, .. } = failure {
		// Another Node did not answer. As with a Decider outage, retry after 2,
		// 4, 8, 16 and 32 s while nothing took effect, then pause until a person
		// resumes, which checks again (#751). A Run never goes on unconfirmed.
		let retries = current.recovery.node_retry + 1;
		if retry && retries <= OUTAGE_RETRIES {
			current.recovery.node_retry = retries;
			current.recovery.retry = Some(RetryState {
				count: retries,
				at: now + chrono::Duration::seconds(2_i64.pow(retries)),
			});
			current.error = Some(failure.message());
			store.save(&current, token, "run.node_retrying").await?;
		} else {
			store
				.pause(
					&current,
					token,
					&failure.message(),
					"run.node_unreachable",
					None,
				)
				.await?;
		}
	} else if let ExecutionFailure::Context(reason) = failure {
		// Final for this step and never a transport retry. Local and remote-hosted
		// Runs share this path: the store records the typed reason, pauses, and
		// emits `run.context_blocked`, like every other execution pause.
		current.recovery.context_reason = Some(reason);
		store.pause_context(&current, token, reason).await?;
	} else if let ExecutionFailure::Authority(authority) = failure {
		// The tool call that raised a named refusal has already named it; any other step is
		// named after the stage the Run was in and the note its checks left.
		let authority = authority.named(&stage(&current), refusal_note::take());
		store
			.pause(
				&current,
				token,
				&authority.reason(),
				"run.authorization_blocked",
				authority.refusal(),
			)
			.await?;
	} else if let ExecutionFailure::MediaRoute(ref message) = failure {
		store
			.pause(&current, token, message, "run.media_route_blocked", None)
			.await?;
	} else if let ExecutionFailure::ToolRetry {
		count,
		retry_after,
		ref message,
	} = failure
		&& current.control != RunControl::Cancelled
		&& !current.state.failure_delivery()
	{
		let backoff = std::time::Duration::from_secs(2_u64.pow(count.min(5)));
		let delay = backoff.max(
			retry_after
				.unwrap_or_default()
				.min(std::time::Duration::from_secs(32)),
		);
		current.recovery.retry = Some(RetryState {
			count,
			at: now + chrono::Duration::seconds(delay.as_secs() as i64),
		});
		current.error = Some(message.clone());
		store.save(&current, token, "run.retrying").await?;
		return Ok(true);
	} else if matches!(
		failure,
		ExecutionFailure::Deferred | ExecutionFailure::DatabaseUnavailable
	) {
		let database = matches!(failure, ExecutionFailure::DatabaseUnavailable);
		current.recovery.retry = Some(RetryState {
			// A database outage spends none of the attempts that bound a real failure.
			count: if database { attempts - 1 } else { attempts },
			at: now + chrono::Duration::seconds(1),
		});
		if database {
			// Say what the Run waits for instead of showing the error of an earlier step.
			current.error = Some(failure.message());
		}
		store.save(&current, token, "run.retrying").await?;
		return Ok(true);
	} else if current.state.failure_delivery() {
		if let RunState::Waiting(wait) = &mut current.state
			&& let WaitingState::FailureDelivery {
				wake_at,
				last_delivery_error,
				..
			} = wait.as_mut()
		{
			*last_delivery_error = Some(failure.message());
			*wake_at = now + chrono::Duration::seconds(5);
		}
		store.save(&current, token, "run.failure_pending").await?;
	} else if failure.retryable() && attempts <= 5 && current.control != RunControl::Cancelled {
		current.recovery.retry = Some(RetryState {
			count: attempts,
			at: now + chrono::Duration::seconds(2_i64.pow(attempts)),
		});
		current.error = Some(failure.message());
		store.save(&current, token, "run.retrying").await?;
		return Ok(true);
	} else if current.control != RunControl::Cancelled
		&& let Some(need) = aidash_application::connection_wait::failure_need(&current)
		&& let Some(policy) = store.connection_wait_policy(&current).await?
	{
		let key = format!(
			"{}:failure:connection:{}",
			current.id,
			aidash_domain::registry::rules::digest(&serde_json::json!(need.resource))
		);
		let request = store
			.connection_request(&current, &need.prompt(), &key)
			.await?;
		let deadline = policy.deadline(request.created_at, 0);
		current
			.recovery
			.connection_waited
			.insert(need.resource.clone());
		current.recovery.connection_wait = Some(aidash_domain::run_state::ConnectionWait {
			request_id: request.id,
			resource: need.resource,
			started_at: request.created_at,
			answer_by: deadline,
			call: None,
			key,
		});
		current.state = RunState::Waiting(Box::new(WaitingState::Human {
			request_id: request.id,
			answer_by: Some(deadline),
			resume: aidash_domain::ResumeState::Thinking(Default::default()),
		}));
		current.error = Some(failure.message());
		store
			.save(&current, token, "run.failure_deferred_for_connection")
			.await?;
	} else {
		let target = if current.control == RunControl::Cancelled {
			FailureTarget::Cancelled
		} else {
			FailureTarget::Failed
		};
		current.state = RunState::Waiting(Box::new(WaitingState::FailureDelivery {
			target,
			wake_at: now,
			last_delivery_error: None,
		}));
		current.error = Some(failure.message());
		store.save(&current, token, "run.failure_pending").await?;
	}
	Ok(false)
}

#[cfg(test)]
mod tests;
