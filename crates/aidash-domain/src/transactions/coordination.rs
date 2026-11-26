//! Participant vote ordering and manifest-bound coordinator decisions.
use super::{
	CoordinatorDecision, CoordinatorTransition,
	authority::{Decision, Status},
	share::Share,
};
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
pub enum ParticipantPhase {
	Reserved,
	Prepared,
	Applied,
	Committed,
	Aborted,
}

impl ParticipantPhase {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Reserved => "RESERVED",
			Self::Prepared => "PREPARED",
			Self::Applied => "APPLIED",
			Self::Committed => "COMMITTED",
			Self::Aborted => "ABORTED",
		}
	}
}

/// A participant holds one share per transaction ID.
pub fn participant_matches(share: &Share, digest: &str, existing: &LocalStatus) -> bool {
	existing.coordinator == share.coordinator
		&& existing.digest == digest
		&& existing.manifest == json!(share)
}

pub fn abortable(existing: &LocalStatus) -> bool {
	!matches!(existing.phase.as_str(), "APPLIED" | "COMMITTED")
}

pub fn commit_ready(existing: &LocalStatus) -> bool {
	matches!(existing.phase.as_str(), "PREPARED" | "APPLIED")
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

/// A participant acknowledges its own share; the digest binds it to every
/// other part.
pub fn validate_acknowledgement(
	share: &Share,
	state: &Status,
	operation: ParticipantOperation,
	response: &LocalStatus,
) -> Result<()> {
	if response.id != share.id
		|| response.coordinator != share.coordinator
		|| response.digest != state.digest
		|| response.manifest != json!(share)
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

/// The digest is computed from the participant's immutable share.
pub fn decision_matches(share: &Share, digest: &str, proof: &Decision) -> bool {
	proof.id == share.id
		&& proof.digest == digest
		&& proof
			.decision
			.as_deref()
			.is_none_or(|decision| matches!(decision, "COMMIT" | "ABORT"))
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
