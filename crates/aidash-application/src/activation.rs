//! Database-owned execution responsibility is independent of transport delivery.
use crate::{Result, ports::activation::*};
use aidash_domain::{
	FailureTarget, Run, RunControl, RunState, WaitingState,
	activation::{Envelope, QuarantineReason},
	run_state::RawRun,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use std::time::Duration;
use uuid::Uuid;

pub const RECONCILE_BATCH_SIZE: u64 = 128;

/// Database time and locked records govern readiness, including malformed state.
pub async fn due(
	scope: &mut dyn SchedulingScope,
	raw: &RawRun,
	now: DateTime<Utc>,
	node: &str,
) -> Result<Option<DateTime<Utc>>> {
	let m = &raw.metadata;
	if m.phase.is_terminal() || m.control == RunControl::Paused {
		return Ok(None);
	}
	if let Some(until) = m.lease_until.filter(|d| *d > now) {
		return Ok(Some(until));
	}
	if m.control == RunControl::Cancelled {
		return Ok(Some(now));
	}
	if !scope.unblocked(m.id).await? {
		return Ok(None);
	}
	let Ok((state, recovery)) = aidash_domain::run_state::decode(m.phase, raw.pending.clone())
	else {
		return Ok(Some(now));
	};
	let retry_at = recovery.retry.map(|r| r.at).filter(|d| *d > now);
	let due = match &state {
		RunState::Waiting(wait) => {
			if let Some(id) = wait.request_id() {
				if scope.human_answered(id).await? {
					Some(now)
				} else {
					wait.deadline()
				}
			} else if let WaitingState::CoreApproval { approval_id, .. } = wait.as_ref() {
				match scope.approval(*approval_id).await? {
					Some(Approval { state, expires_at }) if state == "pending" => expires_at,
					_ => Some(now),
				}
			} else if m.home_node == node
				&& matches!(wait.as_ref(), WaitingState::Dependencies { .. })
			{
				if scope.dependencies_ready(m.task_id).await? {
					Some(now)
				} else {
					wait.deadline()
				}
			} else {
				wait.deadline()
			}
		}
		_ => Some(now),
	};
	let due = if raw.decode().is_err() && !state.failure_delivery() {
		Some(now)
	} else {
		due
	};
	Ok(due.map(|d| retry_at.map_or(d, |r| d.max(r))))
}

/// Targeted notification and keyset recovery share exactly the same eligibility path.
pub async fn lease(
	scope: &mut dyn SchedulingScope,
	token: Uuid,
	seconds: i32,
	target: Option<Uuid>,
	node: &str,
	cursor: &mut RecoveryCursor,
) -> Result<Option<Run>> {
	let now = scope.now().await?;
	let rows = scope.candidates(target, *cursor).await?;
	if rows.is_empty() {
		*cursor = None;
		return Ok(None);
	}
	let more = rows.len() == 128 && target.is_none();
	for raw in rows {
		let m = &raw.metadata;
		*cursor = Some((m.updated_at, m.id));
		if due(scope, &raw, now, node).await?.is_none_or(|d| d > now) {
			continue;
		}
		if aidash_domain::run_state::decode(m.phase, raw.pending.clone())
			.is_ok_and(|(s, _)| s.failure_delivery())
		{
			continue;
		}
		let mut run = match raw.decode() {
			Ok(run) => run,
			Err(error) => {
				let repair_now = scope.now().await?;
				let state = RunState::Waiting(Box::new(WaitingState::FailureDelivery {
					target: if m.control == RunControl::Cancelled {
						FailureTarget::Cancelled
					} else {
						FailureTarget::Failed
					},
					wake_at: repair_now,
					last_delivery_error: None,
				}));
				if let Some(owned) = scope.repair_lease(m, token, seconds).await? {
					scope
						.invalid_state(&owned, token, &state, &error.to_string(), node)
						.await?;
				}
				continue;
			}
		};
		run.recovery.lease_recovered |= m.lease_owner.is_some();
		if let Some(claimed) = scope.lease(&run, token, seconds).await? {
			return Ok(Some(claimed));
		}
	}
	if !more {
		*cursor = None;
	}
	Ok(None)
}

pub enum Handoff {
	Claimed(Box<Run>, Uuid),
	Recorded,
	Invalid,
}

pub async fn claim(repository: &dyn ActivationRepository, envelope: &Envelope) -> Result<Handoff> {
	let mut scope = repository.claim_scope().await?;
	let run = scope.lock_run(envelope.run_id).await?;
	let row = scope.lock_obligation(envelope.activation_id).await?;
	let Some(row) = row.filter(|r| r.matches(envelope)) else {
		return Ok(Handoff::Invalid);
	};
	if row.recorded() {
		scope.commit().await?;
		return Ok(Handoff::Recorded);
	}
	let Some(run) = run else {
		scope.settle(row.id, "run_removed").await?;
		scope.commit().await?;
		return Ok(Handoff::Recorded);
	};
	if run.phase().is_terminal() {
		scope.settle(row.id, "terminal").await?;
		scope.commit().await?;
		return Ok(Handoff::Recorded);
	}
	if run.revision > row.run_revision && scope.newer_transition(&row, &run).await? {
		scope.settle(row.id, "newer_transition").await?;
		scope.commit().await?;
		return Ok(Handoff::Recorded);
	}
	let token = Uuid::new_v4();
	if let Some(claimed) = lease(
		scope.as_mut(),
		token,
		repository.lease_seconds(),
		Some(run.id),
		repository.node_id(),
		&mut None,
	)
	.await?
	{
		scope
			.record_claim(row.id, &claimed, token, repository.lease_seconds())
			.await?;
		scope.commit().await?;
		metrics::counter!("aidash_activation_claims_total", "source" => "notification")
			.increment(1);
		tracing::info!(run_id=%claimed.id, activation_id=%row.id, generation=row.generation, revision=claimed.revision, worker_pid=std::process::id(), "activation lease committed");
		return Ok(Handoff::Claimed(Box::new(claimed), token));
	}
	let now = scope.now().await?;
	let refreshed = scope.read_run(run.id).await?;
	let due = due(scope.as_mut(), &refreshed, now, repository.node_id()).await?;
	scope.defer(row.id, due).await?;
	scope.commit().await?;
	metrics::counter!("aidash_activation_deferred_total").increment(1);
	Ok(Handoff::Recorded)
}

pub enum Publication {
	Published(usize),
	Backpressured,
}

pub async fn publish(
	repository: &dyn ActivationRepository,
	transport: &dyn ActivationTransport,
) -> Result<Publication> {
	if transport.disconnected() {
		return Err(unavailable());
	}
	let mut visibility = repository.visibility().await?;
	let token = Uuid::new_v4();
	let rows = repository.publish_batch(token).await?;
	// A broker acknowledgement never retains the ordinary-state visibility lock.
	visibility.suspend().await?;
	let count = rows.len();
	let mut sends = futures_util::stream::iter(rows.into_iter().map(|row| async move {
		if !transport
			.publish(&row.envelope(repository.node_id()), row.publication_epoch)
			.await?
		{
			return Ok(false);
		}
		repository.published(&row, token).await?;
		metrics::counter!("aidash_activation_published_total").increment(1);
		Result::Ok(true)
	}))
	.buffer_unordered(8);
	let mut backpressured = false;
	while let Some(result) = sends.next().await {
		backpressured |= !result?;
	}
	drop(visibility);
	Ok(if backpressured {
		Publication::Backpressured
	} else {
		Publication::Published(count)
	})
}

/// Retain visibility after committing responsibility until ACK and the worker step finish.
pub struct DeliveryHandoff {
	pub claimed: Option<(Box<Run>, Uuid, Box<dyn VisibilityScope>)>,
	pub progressed: bool,
	pub ack_failed: bool,
}

pub async fn receive(
	repository: &dyn ActivationRepository,
	message: &dyn ActivationDelivery,
	on_committed: impl FnOnce(),
) -> Result<DeliveryHandoff> {
	let envelope = match Envelope::decode(message.payload(), repository.node_id()) {
		Ok(envelope) => envelope,
		Err(reason) => {
			quarantine(repository, message, reason).await?;
			return Ok(DeliveryHandoff {
				claimed: None,
				progressed: false,
				ack_failed: false,
			});
		}
	};
	let disposition = async {
		let visibility = repository.visibility().await?;
		let handoff = claim(repository, &envelope).await?;
		Result::Ok((visibility, handoff))
	}
	.await;
	let (visibility, handoff) = match disposition {
		Ok(value) => value,
		Err(error) => {
			let _ = tokio::time::timeout(Duration::from_secs(1), message.defer()).await;
			return Err(error);
		}
	};
	if matches!(handoff, Handoff::Invalid) {
		quarantine(repository, message, QuarantineReason::InvalidReference).await?;
		return Ok(DeliveryHandoff {
			claimed: None,
			progressed: false,
			ack_failed: false,
		});
	}
	on_committed();
	let ack_failed = !tokio::time::timeout(Duration::from_secs(1), message.acknowledge())
		.await
		.is_ok_and(|r| r.is_ok());
	let claimed = match handoff {
		Handoff::Claimed(run, token) => Some((run, token, visibility)),
		_ => None,
	};
	Ok(DeliveryHandoff {
		claimed,
		progressed: true,
		ack_failed,
	})
}

async fn quarantine(
	repository: &dyn ActivationRepository,
	message: &dyn ActivationDelivery,
	reason: QuarantineReason,
) -> Result<()> {
	repository
		.quarantine(message.payload(), reason, message.sequence())
		.await?;
	message.discard().await
}

pub fn unavailable() -> crate::Error {
	crate::Error::External("activation broker unavailable".into())
}

#[cfg(test)]
mod tests;
