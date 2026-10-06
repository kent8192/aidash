//! Durable participant obligations survive authority revocation and network loss.
use super::coordination::Coordinator;
use crate::{
	Error, Result,
	ports::transactions::participation::{ParticipantRepository, ParticipantScope},
	registry::DefinitionValidation,
};
use aidash_domain::transactions::{
	Manifest,
	coordination::{LocalStatus, ParticipantPhase, abortable, commit_ready, participant_matches},
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Participant {
	repository: Arc<dyn ParticipantRepository>,
	coordinator: Coordinator,
	validation: DefinitionValidation,
}

impl Participant {
	pub fn new(
		repository: Arc<dyn ParticipantRepository>,
		coordinator: Coordinator,
		validation: DefinitionValidation,
	) -> Self {
		Self {
			repository,
			coordinator,
			validation,
		}
	}

	fn sender(&self, caller: &str, manifest: &Manifest) -> Result<()> {
		super::validate(&self.validation, manifest)?;
		manifest
			.local(self.repository.node_id())
			.map_err(|_| Error::Forbidden)?;
		if caller != manifest.coordinator {
			return Err(Error::Forbidden);
		}
		Ok(())
	}

	fn check(existing: &LocalStatus, manifest: &Manifest) -> Result<()> {
		if !participant_matches(manifest, &manifest.digest()?, existing) {
			return Err(Error::Conflict(
				"transaction ID already has another immutable manifest".into(),
			));
		}
		Ok(())
	}

	pub async fn reserve(&self, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
		self.sender(caller, manifest)?;
		// Existing durable admission remains an obligation after live grant revocation.
		let mut scope = self.repository.begin().await?;
		if let Some(existing) = scope.lock(manifest.id).await? {
			Self::check(&existing, manifest)?;
			return scope.finish(Ok(existing)).await;
		}
		scope.rollback().await?;
		if let Some(mut scope) = self.repository.admission(caller, manifest).await? {
			let result = self.reserve_in(scope.as_mut(), caller, manifest).await;
			self.repository
				.fault(manifest.id, "participant.reserve.before")
				.await?;
			let row = scope.finish(result).await?;
			self.repository
				.fault(manifest.id, "participant.reserve.after")
				.await?;
			Ok(row)
		} else {
			let mut scope = self.repository.begin().await?;
			let row = self.reserve_in(scope.as_mut(), caller, manifest).await?;
			self.repository
				.fault(manifest.id, "participant.reserve.before")
				.await?;
			let row = scope.finish(Ok(row)).await?;
			self.repository
				.fault(manifest.id, "participant.reserve.after")
				.await?;
			Ok(row)
		}
	}

	async fn reserve_in(
		&self,
		scope: &mut dyn ParticipantScope,
		caller: &str,
		manifest: &Manifest,
	) -> Result<LocalStatus> {
		if let Some(existing) = scope.lock(manifest.id).await? {
			Self::check(&existing, manifest)?;
			return Ok(existing);
		}
		if caller != self.repository.node_id() && !scope.trusted(caller).await? {
			return Err(Error::Forbidden);
		}
		if scope.gate_exclusive().await?.is_some() {
			return Err(Error::TransactionPending);
		}
		let row = scope.insert(manifest, ParticipantPhase::Reserved).await?;
		scope.reserve_gate(manifest.id).await?;
		scope.reservation_history(manifest.id).await?;
		Ok(row)
	}

	pub async fn prepare(&self, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
		self.sender(caller, manifest)?;
		let mut scope = self.repository.begin().await?;
		let existing = scope
			.lock(manifest.id)
			.await?
			.ok_or_else(|| Error::Conflict("participant was not reserved".into()))?;
		Self::check(&existing, manifest)?;
		if existing.phase != "RESERVED" {
			return scope.finish(Ok(existing)).await;
		}
		if scope.gate_exclusive().await? != Some(manifest.id) {
			return Err(Error::Conflict(
				"participant lost its visibility barrier".into(),
			));
		}
		scope.validate_mutations(manifest).await?;
		let row = scope
			.transition(manifest.id, ParticipantPhase::Prepared)
			.await?;
		self.repository
			.fault(manifest.id, "participant.prepare.before")
			.await?;
		let row = scope.finish(Ok(row)).await?;
		self.repository
			.fault(manifest.id, "participant.prepare.after")
			.await?;
		Ok(row)
	}

	pub async fn finish(&self, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
		self.sender(caller, manifest)?;
		// Participant messages only wake the workflow; its decision comes from storage.
		let proof = self.coordinator.decision(manifest).await?;
		let Some(decision) = proof.decision.as_deref() else {
			return Err(Error::TransactionPending);
		};
		let mut scope = self.repository.begin().await?;
		let existing = scope.lock(manifest.id).await?;
		if let Some(existing) = &existing {
			Self::check(existing, manifest)?;
		}
		if decision == "ABORT" {
			if let Some(existing) = existing {
				if !abortable(&existing) {
					return Err(Error::Conflict("commit cannot be aborted".into()));
				}
				if existing.phase == "ABORTED" {
					return scope.finish(Ok(existing)).await;
				}
				if scope.gate_exclusive().await? != Some(manifest.id) {
					return Err(Error::Conflict(
						"participant lost its visibility barrier".into(),
					));
				}
				scope.release_gate(false).await?;
			} else {
				// The tombstone prevents a delayed reserve from resurrecting aborted work.
				scope.insert(manifest, ParticipantPhase::Aborted).await?;
			}
			let row = scope
				.transition(manifest.id, ParticipantPhase::Aborted)
				.await?;
			self.repository
				.fault(manifest.id, "participant.abort.before")
				.await?;
			let row = scope.finish(Ok(row)).await?;
			self.repository
				.fault(manifest.id, "participant.abort.after")
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
		if scope.gate_exclusive().await? != Some(manifest.id) {
			return Err(Error::Conflict(
				"participant lost its visibility barrier".into(),
			));
		}
		if existing.phase == "PREPARED" {
			scope.apply_mutations(manifest).await?;
			scope
				.transition(manifest.id, ParticipantPhase::Applied)
				.await?;
		}
		let row = if proof.visible {
			scope.release_gate(true).await?;
			scope
				.transition(manifest.id, ParticipantPhase::Committed)
				.await?
		} else {
			scope
				.lock(manifest.id)
				.await?
				.ok_or_else(|| Error::Conflict("participant disappeared during commit".into()))?
		};
		let point = if proof.visible {
			"participant.release"
		} else {
			"participant.apply"
		};
		self.repository
			.fault(manifest.id, &format!("{point}.before"))
			.await?;
		let row = scope.finish(Ok(row)).await?;
		self.repository
			.fault(manifest.id, &format!("{point}.after"))
			.await?;
		self.repository.wake();
		Ok(row)
	}

	pub async fn recover_once(&self) -> Result<usize> {
		let pending = self.repository.pending().await?;
		let mut completed = 0;
		for row in pending.rows() {
			let manifest: Manifest = serde_json::from_value(row.manifest.clone())?;
			// Only a durable coordinator decision can advance a participant's outcome.
			match self.coordinator.decision(&manifest).await {
				Ok(proof) if proof.decision.is_some() => {
					match self.finish(&manifest.coordinator, &manifest).await {
						Ok(_) => completed += 1,
						Err(error) => {
							tracing::debug!(id=%manifest.id,%error,"participant recovery waits")
						}
					}
				}
				Ok(_) => {}
				Err(error) => {
					tracing::debug!(id=%manifest.id,%error,"participant decision unavailable")
				}
			}
		}
		Ok(completed)
	}
}

#[cfg(test)]
mod tests;
