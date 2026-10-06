//! One recoverable coordinator step over durable storage and participant ports.
use crate::{
	Error, Result,
	ports::transactions::coordination::{
		CoordinatorRepository, ParticipantTransport, RecoveryScope,
	},
};
use aidash_domain::transactions::{
	CoordinatorDecision, CoordinatorTransition, Manifest,
	authority::Status,
	coordination::{
		LocalStatus, ParticipantOperation, Vote, decision_matches, next_participant,
		next_transition, validate_acknowledgement,
	},
};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct Coordinator {
	repository: Arc<dyn CoordinatorRepository>,
	transport: Arc<dyn ParticipantTransport>,
}

impl Coordinator {
	pub fn new(
		repository: Arc<dyn CoordinatorRepository>,
		transport: Arc<dyn ParticipantTransport>,
	) -> Self {
		Self {
			repository,
			transport,
		}
	}

	pub async fn status(&self, id: Uuid) -> Result<Status> {
		self.repository.status(id).await
	}

	pub async fn votes(&self, id: Uuid) -> Result<Vec<Vote>> {
		self.repository.votes(id).await
	}

	pub async fn decision(&self, manifest: &Manifest) -> Result<Status> {
		let proof = if manifest.coordinator == self.repository.node_id() {
			self.status(manifest.id).await?
		} else {
			self.transport.decision(manifest).await?
		};
		if !decision_matches(manifest, &manifest.digest()?, &proof) {
			return Err(Error::Conflict(
				"coordinator decision does not match participant manifest".into(),
			));
		}
		Ok(proof)
	}

	pub async fn abort(&self, id: Uuid) -> Result<Status> {
		// Peer I/O holds the recovery lease. Operator abort must still compete for
		// the immutable decision under the storage row lock, without that lease.
		self.repository
			.transition(
				id,
				CoordinatorTransition::Decide(CoordinatorDecision::Abort),
				"operator requested abort",
			)
			.await?;
		let existing = self.status(id).await?;
		if existing.decision.as_deref() == Some("COMMIT") {
			return Err(Error::Conflict("commit is irrevocable".into()));
		}
		Ok(existing)
	}

	/// Recovery repeats this exact step without process-local outcome assumptions.
	pub async fn advance(&self, id: Uuid) -> Result<Status> {
		let mut scope = self.repository.acquire_recovery(id).await?;
		let result = self.advance_locked(scope.as_mut(), id).await;
		scope.release().await?;
		result
	}

	async fn advance_locked(&self, scope: &mut dyn RecoveryScope, id: Uuid) -> Result<Status> {
		let state = self.status(id).await?;
		if state.complete {
			return Ok(state);
		}
		scope.touch().await?;
		let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
		if state.decision.is_none() && manifest.deadline <= self.repository.now() {
			self.repository
				.transition(
					id,
					CoordinatorTransition::Decide(CoordinatorDecision::Abort),
					"deadline elapsed before durable decision",
				)
				.await?;
			return self.status(id).await;
		}
		let votes = self.votes(id).await?;
		if let Some((vote, operation)) = next_participant(&state, &votes) {
			match self.send(&manifest, &vote.node_id, operation).await {
				Ok(response) => {
					validate_acknowledgement(&manifest, &state, operation, &response)?;
					self.repository.fault(id, "coordinator.vote.before").await?;
					self.repository
						.settle_authority(id, &vote.node_id, &response.phase)
						.await?;
					scope.acknowledge(&vote.node_id, &response.phase).await?;
					self.repository.fault(id, "coordinator.vote.after").await?;
				}
				Err(error) => {
					if state.decision.is_none()
						&& (matches!(
							error,
							Error::Conflict(_)
								| Error::Invalid(_) | Error::NotFound(_)
								| Error::Forbidden | Error::Unauthorized
						) || (matches!(error, Error::TransactionPending)
							&& self.repository.scoped(id).await?))
					{
						self.repository
							.transition(
								id,
								CoordinatorTransition::Decide(CoordinatorDecision::Abort),
								&error.to_string(),
							)
							.await?;
					} else {
						scope.record_error(&error.to_string()).await?;
					}
				}
			}
		} else {
			let (change, detail) = next_transition(&state, &votes)?;
			self.repository.transition(id, change, detail).await?;
		}
		self.status(id).await
	}

	async fn send(
		&self,
		manifest: &Manifest,
		node: &str,
		operation: ParticipantOperation,
	) -> Result<LocalStatus> {
		if operation == ParticipantOperation::Reserve {
			self.repository.issue_authority(manifest, node).await?;
		}
		self.transport.send(manifest, node, operation).await
	}

	pub async fn recover_once(&self) -> Result<()> {
		self.recover_kind(false).await?;
		self.recover_kind(true).await
	}

	pub async fn recover_kind(&self, aborted: bool) -> Result<()> {
		let batch = self.repository.recovery_batch(aborted).await?;
		for id in batch.ids() {
			if let Err(error) = self.advance(*id).await {
				tracing::warn!(%id, %error, "atomic transaction recovery pending");
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests;
