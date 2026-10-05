//! Finite domain states shared by business rules and persistence.

use reinhardt::ModelEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum AtomicCoordinatorDecision {
	#[default]
	#[model_enum(value = "COMMIT")]
	#[serde(rename = "COMMIT")]
	Commit,
	#[model_enum(value = "ABORT")]
	#[serde(rename = "ABORT")]
	Abort,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum AtomicHistoryRole {
	#[default]
	#[model_enum(value = "coordinator")]
	#[serde(rename = "coordinator")]
	Coordinator,
	#[model_enum(value = "participant")]
	#[serde(rename = "participant")]
	Participant,
	#[model_enum(value = "trust")]
	#[serde(rename = "trust")]
	Trust,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum AtomicParticipantPhase {
	#[default]
	#[model_enum(value = "RESERVED")]
	#[serde(rename = "RESERVED")]
	Reserved,
	#[model_enum(value = "PREPARED")]
	#[serde(rename = "PREPARED")]
	Prepared,
	#[model_enum(value = "APPLIED")]
	#[serde(rename = "APPLIED")]
	Applied,
	#[model_enum(value = "COMMITTED")]
	#[serde(rename = "COMMITTED")]
	Committed,
	#[model_enum(value = "ABORTED")]
	#[serde(rename = "ABORTED")]
	Aborted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, ModelEnum)]
#[model_enum(repr = "string")]
pub enum AtomicVotePhase {
	#[default]
	#[model_enum(value = "PENDING")]
	#[serde(rename = "PENDING")]
	Pending,
	#[model_enum(value = "RESERVED")]
	#[serde(rename = "RESERVED")]
	Reserved,
	#[model_enum(value = "PREPARED")]
	#[serde(rename = "PREPARED")]
	Prepared,
	#[model_enum(value = "APPLIED")]
	#[serde(rename = "APPLIED")]
	Applied,
	#[model_enum(value = "COMMITTED")]
	#[serde(rename = "COMMITTED")]
	Committed,
	#[model_enum(value = "ABORTED")]
	#[serde(rename = "ABORTED")]
	Aborted,
}

impl AtomicVotePhase {
	pub(crate) fn as_str(&self) -> &'static str {
		match self {
			Self::Pending => "PENDING",
			Self::Reserved => "RESERVED",
			Self::Prepared => "PREPARED",
			Self::Applied => "APPLIED",
			Self::Committed => "COMMITTED",
			Self::Aborted => "ABORTED",
		}
	}
}

impl AtomicParticipantPhase {
	pub(crate) fn as_str(&self) -> &'static str {
		match self {
			Self::Reserved => "RESERVED",
			Self::Prepared => "PREPARED",
			Self::Applied => "APPLIED",
			Self::Committed => "COMMITTED",
			Self::Aborted => "ABORTED",
		}
	}
}
