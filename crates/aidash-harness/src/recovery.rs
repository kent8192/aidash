//! Durable worker recovery. Retrying an outbox never repeats a failed tool.
use aidash_application::{Result, ports::ExecutionRecoveryStore, recovery::ExecutionFailure};
use aidash_domain::semantic::{Failure as SemanticFailure, OUTAGE_RETRIES, PENDING_REPLAY_SECONDS};
use aidash_domain::{FailureTarget, RetryState, RunControl, RunState, WaitingState};
use chrono::{DateTime, Utc};
use uuid::Uuid;

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
	} else if let ExecutionFailure::Context(reason) = failure {
		// Final for this step and never a transport retry. Local and remote-hosted
		// Runs share this path: the store records the typed reason, pauses, and
		// emits `run.context_blocked`, like every other execution pause.
		current.recovery.context_reason = Some(reason);
		store.pause_context(&current, token, reason).await?;
	} else if let ExecutionFailure::Authority(authority) = failure {
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
	} else if matches!(failure, ExecutionFailure::Deferred) {
		current.recovery.retry = Some(RetryState {
			count: attempts,
			at: now + chrono::Duration::seconds(1),
		});
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
