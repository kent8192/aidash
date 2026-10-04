//! Participant vote ordering and manifest-bound coordinator decisions.
use super::{CoordinatorDecision, CoordinatorTransition, Manifest, authority::Status};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vote {
	pub node_id: String,
	pub phase: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalStatus {
	pub id: Uuid,
	pub coordinator: String,
	pub digest: String,
	pub manifest: Value,
	pub phase: String,
	pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticipantOperation {
	Reserve,
	Prepare,
	Finish,
}

impl ParticipantOperation {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Reserve => "reserve",
			Self::Prepare => "prepare",
			Self::Finish => "finish",
		}
	}

	pub fn expected_phase(self, state: &Status) -> &'static str {
		match self {
			Self::Reserve => "RESERVED",
			Self::Prepare => "PREPARED",
			Self::Finish if state.decision.as_deref() == Some("ABORT") => "ABORTED",
			Self::Finish if state.visible => "COMMITTED",
			Self::Finish => "APPLIED",
		}
	}
}

/// Reserve every pending participant before preparing the first reservation.
/// Commit publication waits for all applications; finalization waits for visibility.
pub fn next_participant<'a>(
	state: &Status,
	votes: &'a [Vote],
) -> Option<(&'a Vote, ParticipantOperation)> {
	if state.decision.is_none() {
		votes
			.iter()
			.find(|vote| vote.phase == "PENDING")
			.map(|vote| (vote, ParticipantOperation::Reserve))
			.or_else(|| {
				votes
					.iter()
					.find(|vote| vote.phase == "RESERVED")
					.map(|vote| (vote, ParticipantOperation::Prepare))
			})
	} else if state.decision.as_deref() == Some("ABORT") {
		votes
			.iter()
			.find(|vote| vote.phase != "ABORTED")
			.map(|vote| (vote, ParticipantOperation::Finish))
	} else if !state.visible {
		votes
			.iter()
			.find(|vote| !matches!(vote.phase.as_str(), "APPLIED" | "COMMITTED"))
			.map(|vote| (vote, ParticipantOperation::Finish))
	} else {
		votes
			.iter()
			.find(|vote| vote.phase != "COMMITTED")
			.map(|vote| (vote, ParticipantOperation::Finish))
	}
}

pub fn validate_acknowledgement(
	manifest: &Manifest,
	state: &Status,
	operation: ParticipantOperation,
	response: &LocalStatus,
) -> Result<()> {
	if response.id != manifest.id
		|| response.coordinator != manifest.coordinator
		|| response.digest != state.digest
		|| response.manifest != state.manifest
	{
		return Err(Error::Conflict(
			"participant acknowledged another manifest".into(),
		));
	}
	if response.phase != operation.expected_phase(state) {
		return Err(Error::Conflict(
			"participant returned an unexpected phase".into(),
		));
	}
	Ok(())
}

/// The digest is computed from the participant's immutable manifest.
pub fn decision_matches(manifest: &Manifest, digest: &str, proof: &Status) -> bool {
	proof.id == manifest.id
		&& proof.digest == digest
		&& proof.manifest == json!(manifest)
		&& !proof
			.decision
			.as_deref()
			.is_some_and(|decision| !matches!(decision, "COMMIT" | "ABORT"))
		&& (!proof.visible || proof.decision.as_deref() == Some("COMMIT"))
}

/// Called only after every selected participant operation has settled.
pub fn next_transition(
	state: &Status,
	votes: &[Vote],
) -> Result<(CoordinatorTransition, &'static str)> {
	if state.decision.is_none() {
		if !votes.iter().all(|vote| vote.phase == "PREPARED") {
			return Err(Error::Conflict(
				"commit requires every prepared vote".into(),
			));
		}
		Ok((
			CoordinatorTransition::Decide(CoordinatorDecision::Commit),
			"",
		))
	} else if state.decision.as_deref() == Some("COMMIT") && !state.visible {
		Ok((
			CoordinatorTransition::Publish,
			"every participant durably applied commit",
		))
	} else {
		Ok((
			CoordinatorTransition::Complete,
			"every participant finalized",
		))
	}
}

#[cfg(test)]
mod tests;
