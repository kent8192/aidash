//! Durable scheduling references never authorize execution or carry Run state.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
	pub version: u32,
	pub node_id: String,
	pub run_id: Uuid,
	pub activation_id: Uuid,
	pub generation: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
	Malformed,
	UnsupportedVersion,
	WrongScope,
	InvalidReference,
}

impl QuarantineReason {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Malformed => "malformed",
			Self::UnsupportedVersion => "unsupported_version",
			Self::WrongScope => "wrong_scope",
			Self::InvalidReference => "invalid_reference",
		}
	}
}

impl Envelope {
	pub fn decode(payload: &[u8], node: &str) -> Result<Self, QuarantineReason> {
		let value: Self =
			serde_json::from_slice(payload).map_err(|_| QuarantineReason::Malformed)?;
		if value.version != 1 {
			return Err(QuarantineReason::UnsupportedVersion);
		}
		if value.node_id != node || value.generation <= 0 {
			return Err(QuarantineReason::WrongScope);
		}
		Ok(value)
	}
}

/// A projection of a locked database obligation, separate from its ORM record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Obligation {
	pub id: Uuid,
	pub generation: i64,
	pub run_id: Uuid,
	pub run_revision: i64,
	pub state: String,
	pub publication_epoch: i64,
}

impl Obligation {
	pub fn envelope(&self, node: &str) -> Envelope {
		Envelope {
			version: 1,
			node_id: node.into(),
			run_id: self.run_id,
			activation_id: self.id,
			generation: self.generation,
		}
	}

	pub fn matches(&self, envelope: &Envelope) -> bool {
		self.run_id == envelope.run_id && self.generation == envelope.generation
	}

	/// A delivery that finds its obligation claimed, settled or deferred was already judged.
	/// Republication returns a deferred obligation to `pending` first, so a delivery that
	/// still finds it deferred was published before that deferral and carries nothing new.
	pub fn recorded(&self) -> bool {
		matches!(self.state.as_str(), "settled" | "claimed" | "deferred")
	}
}

#[cfg(test)]
mod tests;
