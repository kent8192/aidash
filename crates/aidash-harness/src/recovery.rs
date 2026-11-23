//! Durable worker recovery. Retrying an outbox never repeats a failed tool.
use aidash_application::{
	Result, authorization::refusal_note, ports::ExecutionRecoveryStore, recovery::ExecutionFailure,
};
use aidash_domain::run_state::{OutageCadence, OutageTracking};
use aidash_domain::semantic::{Failure as SemanticFailure, OUTAGE_RETRIES, PENDING_REPLAY_SECONDS};
use aidash_domain::{FailureTarget, RetryState, Run, RunControl, RunPhase, RunState, WaitingState};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The service an Outage Wait names in the dashboard ("Outage Wait: <service>").
const DECIDER: &str = "Decider";
const SEMANTIC_CONTEXT: &str = "Semantic context";
const PLAN_ALLOWANCE: &str = "ChatGPT plan usage limit";
/// What a Run sleeps on while a Budget is at its Hard Limit (#1012).
const BUDGET: &str = "Budget";

fn track_outage(run: &mut Run, service: &str, now: DateTime<Utc>) -> DateTime<Utc> {
	if run
		.recovery
		.outage
		.as_ref()
		.is_none_or(|outage| outage.service != service)
	{
		run.recovery.outage = Some(OutageTracking {
			service: service.into(),
			started_at: now,
		});
	}
	run.recovery
		.outage
		.as_ref()
		.expect("outage just recorded")
		.started_at
}

/// Wait out `service` until its next check (ADR 0129). Returns `false` once the
/// outage has reached its bound; the caller then pauses the Run.
///
/// `spent` is the generic retry count to keep while waiting: the short retries
/// of an outage counted by `recovery.retry` (semantic context, a service). The
/// wake-up restores it, so the next failure waits again at once instead of
/// retrying for another minute. A Decider or Node outage keeps its own count.
///
/// A check at worker entry can fail while the Run still waits, as a wait wakes
/// and before its step runs. The Run then keeps what it waits for: an Outage
/// Wait moves its next check and names the service failing now, and any other
/// wait keeps its state while its wake-up is held until that check.
async fn wait_outage(
	store: &dyn ExecutionRecoveryStore,
	run: &mut Run,
	token: Uuid,
	service: &str,
	cadence: OutageCadence,
	spent: Option<u32>,
	now: DateTime<Utc>,
) -> Result<bool> {
	let started_at = track_outage(run, service, now);
	let Some(wake_at) = cadence.next_wake(started_at, now) else {
		return Ok(false);
	};
	// In an Outage Wait the state's `wake_at` schedules the next check, so the
	// kept count carries no later time of its own (`activation::due` takes the
	// later of the two).
	let spent = spent.map(|count| RetryState { count, at: now });
	if let RunState::Waiting(wait) = &mut run.state {
		if let WaitingState::Outage {
			service: waiting_for,
			started_at: since,
			wake_at: next,
			cadence: every,
			..
		} = wait.as_mut()
		{
			*waiting_for = service.into();
			*since = started_at;
			*next = wake_at;
			*every = cadence;
			run.recovery.retry = spent;
		} else {
			// Activation wakes a wait no earlier than its retry (`activation::due`).
			run.recovery.retry = Some(RetryState {
				count: spent.map_or(0, |retry| retry.count),
				at: wake_at,
			});
		}
	} else {
		run.suspend(|resume| WaitingState::Outage {
			service: service.into(),
			started_at,
			wake_at,
			resume,
			cadence,
		})?;
		run.recovery.retry = spent;
	}
	store.save(run, token, "execution.outage_waiting").await?;
	Ok(true)
}

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
		if reason == SemanticFailure::Unavailable {
			track_outage(&mut current, SEMANTIC_CONTEXT, now);
		}
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
			store
				.save(&current, token, "execution.semantic_retrying")
				.await?;
		} else if reason == SemanticFailure::Pending
			|| (reason.transient() && attempts <= OUTAGE_RETRIES)
		{
			let delay = if reason == SemanticFailure::Pending {
				// Each replay repeats the whole semantic-context step, including
				// its charged thread embedding, so only the first check of a
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
			store
				.save(&current, token, "execution.semantic_retrying")
				.await?;
		} else if reason == SemanticFailure::Unavailable {
			// The Run still retries by itself, so it stays `Unavailable` with a
			// scheduled retry and management offers only `wait`: nobody is told
			// to act before the bound (ADR 0129). The bound pauses it as
			// `RetriesExhausted`.
			current.recovery.semantic_reason = Some(reason);
			if !wait_outage(
				store,
				&mut current,
				token,
				SEMANTIC_CONTEXT,
				OutageCadence::Service,
				Some(OUTAGE_RETRIES),
				now,
			)
			.await?
			{
				current.recovery.semantic_reason = Some(SemanticFailure::RetriesExhausted);
				store
					.pause_semantic(&current, token, SemanticFailure::RetriesExhausted)
					.await?;
			}
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
		track_outage(&mut current, DECIDER, now);
		// Every Decider Route failed. Retry after 2, 4, 8, 16 and 32 s, each
		// time as a new decision, then pause until a person resumes (ADR 0066).
		let retries = current.recovery.decider_retry.saturating_add(1);
		if retries <= OUTAGE_RETRIES {
			current.recovery.decider_retry = retries;
			current.recovery.retry = Some(RetryState {
				count: retries,
				at: now + chrono::Duration::seconds(2_i64.pow(retries)),
			});
			current.error = Some(failure.message());
			store
				.save(&current, token, "execution.decider_retrying")
				.await?;
		} else {
			current.recovery.decider_retry = retries;
			let reason = aidash_domain::context::recovery::Failure::DeciderUnavailable;
			current.recovery.context_reason = Some(reason);
			if !wait_outage(
				store,
				&mut current,
				token,
				DECIDER,
				OutageCadence::Service,
				None,
				now,
			)
			.await?
			{
				store.pause_context(&current, token, reason).await?;
			}
		}
	} else if let ExecutionFailure::NodeUnreachable { ref node, .. } = failure {
		let service = format!("Node {node}");
		track_outage(&mut current, &service, now);
		// Another Node did not answer. As with a Decider outage, retry after 2,
		// 4, 8, 16 and 32 s, then wait out the outage (ADR 0129). A Run never
		// goes on unconfirmed (#751). After a tool's effect (`retry` false) the
		// step is not run again: its invocation stays STARTED, and taking the
		// step up again reconciles it, exactly as a manual resume used to.
		let retries = current.recovery.node_retry.saturating_add(1);
		if retries <= OUTAGE_RETRIES {
			current.recovery.node_retry = retries;
			current.recovery.retry = Some(RetryState {
				count: retries,
				at: now + chrono::Duration::seconds(2_i64.pow(retries)),
			});
			current.error = Some(failure.message());
			store
				.save(&current, token, "execution.node_retrying")
				.await?;
		} else if wait_outage(
			store,
			&mut current,
			token,
			&service,
			OutageCadence::Service,
			None,
			now,
		)
		.await?
		{
			// Waiting; the Run goes on by itself once the Node answers.
		} else {
			store
				.pause(
					&current,
					token,
					&failure.message(),
					"execution.node_unreachable",
					None,
				)
				.await?;
		}
	} else if let ExecutionFailure::Context(reason) = failure {
		// Final for this step and never a transport retry. Local and remote-hosted
		// Runs share this path: the store records the typed reason, pauses, and
		// emits `execution.context_blocked`, like every other execution pause.
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
				"execution.authorization_blocked",
				authority.refusal(),
			)
			.await?;
	} else if let ExecutionFailure::ServiceOutage(ref service) = failure {
		// A service that will come back by itself: retry after 2, 4, 8, 16
		// and 32 s, then wait it out (ADR 0129).
		track_outage(&mut current, service, now);
		current.error = Some(failure.message());
		if attempts <= OUTAGE_RETRIES {
			current.recovery.retry = Some(RetryState {
				count: attempts,
				at: now + chrono::Duration::seconds(2_i64.pow(attempts)),
			});
			store.save(&current, token, "execution.retrying").await?;
			return Ok(true);
		}
		if !wait_outage(
			store,
			&mut current,
			token,
			service,
			OutageCadence::Service,
			Some(OUTAGE_RETRIES),
			now,
		)
		.await?
		{
			store
				.pause(
					&current,
					token,
					&failure.message(),
					"execution.service_unavailable",
					None,
				)
				.await?;
		}
	} else if let ExecutionFailure::Budget { stop, ref message } = failure {
		// A Budget at its Hard Limit is a person's money decision, never a
		// failure: stop for them, or sleep until it has room again (#1012).
		current.recovery.retry = None;
		current.error = Some(message.clone());
		if stop
			|| !wait_outage(
				store,
				&mut current,
				token,
				BUDGET,
				OutageCadence::Budget,
				None,
				now,
			)
			.await?
		{
			store
				.pause(&current, token, message, "execution.budget_exhausted", None)
				.await?;
		}
	} else if let ExecutionFailure::PlanAllowance(ref message) = failure {
		// The plan's usage limit lifts by itself at a time OpenAI does not give,
		// so the Run waits and tries again every hour (ADR 0129).
		current.error = Some(message.clone());
		if !wait_outage(
			store,
			&mut current,
			token,
			PLAN_ALLOWANCE,
			OutageCadence::PlanAllowance,
			None,
			now,
		)
		.await?
		{
			store
				.pause(
					&current,
					token,
					message,
					"execution.plan_allowance_exhausted",
					None,
				)
				.await?;
		}
	} else if let ExecutionFailure::MediaRoute(ref message) = failure {
		store
			.pause(
				&current,
				token,
				message,
				"execution.media_route_blocked",
				None,
			)
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
		store.save(&current, token, "execution.retrying").await?;
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
		store.save(&current, token, "execution.retrying").await?;
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
		store
			.save(&current, token, "execution.failure_pending")
			.await?;
	} else if failure.retryable() && attempts <= 5 && current.control != RunControl::Cancelled {
		current.recovery.retry = Some(RetryState {
			count: attempts,
			at: now + chrono::Duration::seconds(2_i64.pow(attempts)),
		});
		current.error = Some(failure.message());
		store.save(&current, token, "execution.retrying").await?;
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
			.save(&current, token, "execution.failure_deferred_for_connection")
			.await?;
	} else {
		let target = if current.control == RunControl::Cancelled {
			FailureTarget::Cancelled
		} else if matches!(failure, ExecutionFailure::QueueLimit) {
			FailureTarget::QueueLimit
		} else {
			FailureTarget::Failed
		};
		current.state = RunState::Waiting(Box::new(WaitingState::FailureDelivery {
			target,
			wake_at: now,
			last_delivery_error: None,
		}));
		current.error = Some(failure.message());
		store
			.save(&current, token, "execution.failure_pending")
			.await?;
	}
	Ok(false)
}

#[cfg(test)]
mod tests;
