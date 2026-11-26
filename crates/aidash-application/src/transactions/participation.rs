//! Durable participant obligations survive authority revocation and network loss.
use super::coordination::Coordinator;
use crate::{
	Error, Result,
	ports::transactions::participation::{ParticipantRepository, ParticipantScope},
};
use aidash_domain::transactions::{
	coordination::{LocalStatus, ParticipantPhase, abortable, commit_ready, participant_matches},
	share::Share,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Participant {
	repository: Arc<dyn ParticipantRepository>,
	coordinator: Coordinator,
}

impl Participant {
	pub fn new(repository: Arc<dyn ParticipantRepository>, coordinator: Coordinator) -> Self {
		Self {
			repository,
			coordinator,
		}
	}

	fn sender(&self, caller: &str, share: &Share) -> Result<()> {
		super::validate_share(share)?;
		// An inbound request is for the Node the transport admitted. Work this
		// Deployment does for itself, such as recovery, is for its one Node in
		// the transaction (`Manifest::validate` admits no other).
		let own = match self.repository.receiver() {
			Some(receiver) => share.part.node_id == receiver,
			None => aidash_domain::nodes::same_deployment(
				&share.part.node_id,
				self.repository.node_id(),
			),
		};
		if !own {
			return Err(Error::Forbidden);
		}
		if caller != share.coordinator {
			return Err(Error::Forbidden);
		}
		Ok(())
	}

	fn check(existing: &LocalStatus, share: &Share) -> Result<()> {
		if !participant_matches(share, &share.digest()?, existing) {
			return Err(Error::Conflict(
				"transaction ID already has another immutable manifest".into(),
			));
		}
		Ok(())
	}

	async fn expire_reserved(
		scope: &mut dyn ParticipantScope,
		share: &Share,
		existing: &LocalStatus,
	) -> Result<Option<LocalStatus>> {
		if existing.phase != "RESERVED" || share.deadline > scope.now().await? {
			return Ok(None);
		}
		if scope.gate_exclusive().await? != Some(share.id) {
			return Err(Error::Conflict(
				"participant lost its visibility barrier".into(),
			));
		}
		Self::abort_reserved(scope, share).await.map(Some)
	}

	/// Abort a RESERVED participant whose deadline passed; the caller holds its gate.
	async fn abort_reserved(
		scope: &mut dyn ParticipantScope,
		share: &Share,
	) -> Result<LocalStatus> {
		scope.release_gate(false).await?;
		scope.transition(share.id, ParticipantPhase::Aborted).await
	}

	pub async fn reserve(&self, caller: &str, share: &Share) -> Result<LocalStatus> {
		self.sender(caller, share)?;
		// Existing durable admission remains an obligation after live grant revocation.
		let mut scope = self.repository.begin().await?;
		if let Some(existing) = scope.lock(share.id).await? {
			Self::check(&existing, share)?;
			let row = Self::expire_reserved(scope.as_mut(), share, &existing)
				.await?
				.unwrap_or(existing);
			return scope.finish(Ok(row)).await;
		}
		scope.rollback().await?;
		if let Some(mut scope) = self.repository.admission(caller, share).await? {
			let result = self.reserve_in(scope.as_mut(), caller, share).await;
			self.repository
				.fault(share.id, "participant.reserve.before")
				.await?;
			let row = scope.finish(result).await?;
			self.repository
				.fault(share.id, "participant.reserve.after")
				.await?;
			Ok(row)
		} else {
			let mut scope = self.repository.begin().await?;
			let row = self.reserve_in(scope.as_mut(), caller, share).await?;
			self.repository
				.fault(share.id, "participant.reserve.before")
				.await?;
			let row = scope.finish(Ok(row)).await?;
			self.repository
				.fault(share.id, "participant.reserve.after")
				.await?;
			Ok(row)
		}
	}

	async fn reserve_in(
		&self,
		scope: &mut dyn ParticipantScope,
		_caller: &str,
		share: &Share,
	) -> Result<LocalStatus> {
		if let Some(existing) = scope.lock(share.id).await? {
			Self::check(&existing, share)?;
			return Ok(Self::expire_reserved(scope, share, &existing)
				.await?
				.unwrap_or(existing));
		}
		let remaining = share
			.deadline
			.signed_duration_since(scope.now().await?)
			.num_seconds();
		if !(1..=3600).contains(&remaining) {
			return Err(Error::Invalid(
				"new participant deadline must be within the next hour".into(),
			));
		}
		if scope.gate_exclusive().await?.is_some() {
			return Err(Error::TransactionPending);
		}
		// Waiting for the gate can outlast the deadline sampled before it.
		if share.deadline <= scope.now().await? {
			return Err(Error::Invalid(
				"participant deadline passed while waiting for the visibility barrier".into(),
			));
		}
		let row = scope.insert(share, ParticipantPhase::Reserved).await?;
		scope.reserve_gate(share.id).await?;
		scope.reservation_history(share.id).await?;
		Ok(row)
	}

	pub async fn prepare(&self, caller: &str, share: &Share) -> Result<LocalStatus> {
		self.sender(caller, share)?;
		let mut scope = self.repository.begin().await?;
		let existing = scope
			.lock(share.id)
			.await?
			.ok_or_else(|| Error::Conflict("participant was not reserved".into()))?;
		Self::check(&existing, share)?;
		if existing.phase != "RESERVED" {
			return scope.finish(Ok(existing)).await;
		}
		if scope.gate_exclusive().await? != Some(share.id) {
			return Err(Error::Conflict(
				"participant lost its visibility barrier".into(),
			));
		}
		// Sample the deadline only once the gate is held: waiting for it can
		// outlast a deadline that was still in the future before the wait.
		if share.deadline <= scope.now().await? {
			let aborted = Self::abort_reserved(scope.as_mut(), share).await?;
			return scope.finish(Ok(aborted)).await;
		}
		scope.validate_mutations(share).await?;
		let row = scope
			.transition(share.id, ParticipantPhase::Prepared)
			.await?;
		self.repository
			.fault(share.id, "participant.prepare.before")
			.await?;
		let row = scope.finish(Ok(row)).await?;
		self.repository
			.fault(share.id, "participant.prepare.after")
			.await?;
		Ok(row)
	}

	pub async fn finish(&self, caller: &str, share: &Share) -> Result<LocalStatus> {
		self.sender(caller, share)?;
		// Participant messages only wake the workflow; its decision comes from storage.
		let proof = self.coordinator.decision(share).await?;
		let Some(decision) = proof.decision.as_deref() else {
			return Err(Error::TransactionPending);
		};
		let mut scope = self.repository.begin().await?;
		let existing = scope.lock(share.id).await?;
		if let Some(existing) = &existing {
			Self::check(existing, share)?;
		}
		if decision == "ABORT" {
			if let Some(existing) = existing {
				if !abortable(&existing) {
					return Err(Error::Conflict("commit cannot be aborted".into()));
				}
				if existing.phase == "ABORTED" {
					return scope.finish(Ok(existing)).await;
				}
				if scope.gate_exclusive().await? != Some(share.id) {
					return Err(Error::Conflict(
						"participant lost its visibility barrier".into(),
					));
				}
				scope.release_gate(false).await?;
			} else {
				// The tombstone prevents a delayed reserve from resurrecting aborted work.
				scope.insert(share, ParticipantPhase::Aborted).await?;
			}
			let row = scope
				.transition(share.id, ParticipantPhase::Aborted)
				.await?;
			self.repository
				.fault(share.id, "participant.abort.before")
				.await?;
			let row = scope.finish(Ok(row)).await?;
			self.repository
				.fault(share.id, "participant.abort.after")
				.await?;
			return Ok(row);
		}
		let existing = existing
			.ok_or_else(|| Error::Conflict("commit requires a prepared participant".into()))?;
		if existing.phase == "COMMITTED" {
			return scope.finish(Ok(existing)).await;
		}
		if !commit_ready(&existing) {
			return Err(Error::Conflict(
				"commit requires a prepared participant".into(),
			));
		}
		if scope.gate_exclusive().await? != Some(share.id) {
			return Err(Error::Conflict(
				"participant lost its visibility barrier".into(),
			));
		}
		if existing.phase == "PREPARED" {
			scope.apply_mutations(share).await?;
			scope
				.transition(share.id, ParticipantPhase::Applied)
				.await?;
		}
		let row = if proof.visible {
			scope.release_gate(true).await?;
			scope
				.transition(share.id, ParticipantPhase::Committed)
				.await?
		} else {
			scope
				.lock(share.id)
				.await?
				.ok_or_else(|| Error::Conflict("participant disappeared during commit".into()))?
		};
		let point = if proof.visible {
			"participant.release"
		} else {
			"participant.apply"
		};
		self.repository
			.fault(share.id, &format!("{point}.before"))
			.await?;
		let row = scope.finish(Ok(row)).await?;
		self.repository
			.fault(share.id, &format!("{point}.after"))
			.await?;
		self.repository.wake();
		Ok(row)
	}

	pub async fn recover_once(&self) -> Result<usize> {
		let pending = self.repository.pending().await?;
		let mut completed = 0;
		for row in pending.rows() {
			let share: Share = serde_json::from_value(row.manifest.clone())?;
			if row.phase == "RESERVED" {
				let mut scope = self.repository.begin().await?;
				if let Some(existing) = scope.lock(share.id).await? {
					Self::check(&existing, &share)?;
					if let Some(aborted) =
						Self::expire_reserved(scope.as_mut(), &share, &existing).await?
					{
						scope.finish(Ok(aborted)).await?;
						completed += 1;
						continue;
					}
				}
				scope.rollback().await?;
			}
			// Only a durable coordinator decision can advance a participant's outcome.
			match self.coordinator.decision(&share).await {
				Ok(proof) if proof.decision.is_some() => {
					match self.finish(&share.coordinator, &share).await {
						Ok(_) => completed += 1,
						Err(error) => {
							tracing::debug!(id=%share.id,%error,"participant recovery waits")
						}
					}
				}
				Ok(_) => {}
				Err(error) => {
					tracing::debug!(id=%share.id,%error,"participant decision unavailable")
				}
			}
		}
		Ok(completed)
	}
}

#[cfg(test)]
mod tests;
