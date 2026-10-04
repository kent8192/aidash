//! Durable worker recovery. Retrying an outbox never repeats a failed tool.
use crate::{Result, ports::ExecutionRecoveryStore};
use aidash_domain::semantic::Failure as SemanticFailure;
use aidash_domain::{FailureTarget, RetryState, RunControl, RunState, WaitingState};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Adapter classification retains transport details outside the use case.
#[derive(Debug)]
pub enum ExecutionFailure {
	Semantic(SemanticFailure),
	Authority {
		identity_unavailable: bool,
	},
	MediaRoute(String),
	Deferred,
	Inference {
		transport: bool,
		status: Option<u16>,
		message: String,
	},
	Other(String),
}
impl ExecutionFailure {
	pub fn retryable(&self) -> bool {
		matches!(
			self,
			Self::Inference {
				transport: true,
				..
			}
		) || matches!(
			self,
			Self::Inference {
				status: Some(408 | 429 | 500..=599),
				..
			}
		)
	}
	fn message(&self) -> String {
		match self {
			Self::Semantic(reason) => reason.to_string(),
			Self::Authority {
				identity_unavailable: true,
			} => "external identity status is unavailable".into(),
			Self::Authority {
				identity_unavailable: false,
			} => "execution authority denied".into(),
			Self::MediaRoute(message) | Self::Inference { message, .. } | Self::Other(message) => {
				message.clone()
			}
			Self::Deferred => "execution state is temporarily unavailable".into(),
		}
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
		if reason == SemanticFailure::Pending || (reason.transient() && attempts <= 5) {
			let delay = if reason == SemanticFailure::Pending {
				1
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
			let reason = if reason.transient() {
				SemanticFailure::RetriesExhausted
			} else {
				reason
			};
			current.recovery.semantic_reason = Some(reason);
			store.pause_semantic(&current, token, reason).await?;
		}
	} else if let ExecutionFailure::Authority {
		identity_unavailable,
	} = failure
	{
		let reason = if identity_unavailable {
			"identity status unavailable"
		} else {
			"execution authority denied"
		};
		store
			.pause(&current, token, reason, "run.authorization_blocked")
			.await?;
	} else if let ExecutionFailure::MediaRoute(ref message) = failure {
		store
			.pause(&current, token, message, "run.media_route_blocked")
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
