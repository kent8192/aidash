//! Safety Stop: a Deployment Administrator's pause or cancellation of one Run
//! this Deployment executes, named by Tenant and Run, always with a reason and
//! without seeing the Run's content (ADR 0101). It never resumes a Run.
use crate::{
	Error, Result,
	entities::{RunControl, RunPhase},
	research::revocation::bounded_reason,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// What a Safety Stop asks of the Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
	Pause,
	Cancel,
}

impl Action {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Pause => "pause",
			Self::Cancel => "cancel",
		}
	}

	/// The control the Run holds once the action is applied.
	pub fn control(self) -> RunControl {
		match self {
			Self::Pause => RunControl::Paused,
			Self::Cancel => RunControl::Cancelled,
		}
	}
}

impl TryFrom<&str> for Action {
	type Error = Error;
	fn try_from(value: &str) -> Result<Self> {
		match value {
			"pause" => Ok(Self::Pause),
			"cancel" => Ok(Self::Cancel),
			other => Err(Error::Invalid(format!(
				"unknown Safety Stop action {other}"
			))),
		}
	}
}

/// Who issued the command. The shared recovery token identifies no person and
/// is never recorded as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
	SharedBearer,
	Browser { identity: Uuid },
}

impl Actor {
	/// The actor kind recorded with the command.
	pub fn kind(self) -> &'static str {
		match self {
			Self::SharedBearer => "shared_bearer",
			Self::Browser { .. } => "browser",
		}
	}

	/// The browser Identity, when a named administrator acted.
	pub fn identity(self) -> Option<Uuid> {
		match self {
			Self::SharedBearer => None,
			Self::Browser { identity } => Some(identity),
		}
	}

	/// The scope of an operation ID. Every holder of the shared token shares one
	/// scope; each named administrator has their own.
	pub fn caller_key(self) -> String {
		match self {
			Self::SharedBearer => "shared_bearer".into(),
			Self::Browser { identity } => format!("browser:{identity}"),
		}
	}
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	/// The control change was recorded.
	Accepted,
	/// The Run had already ended or its cancellation was already requested.
	NoOp,
}

impl Outcome {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Accepted => "accepted",
			Self::NoOp => "no_op",
		}
	}
}

impl TryFrom<&str> for Outcome {
	type Error = Error;
	fn try_from(value: &str) -> Result<Self> {
		match value {
			"accepted" => Ok(Self::Accepted),
			"no_op" => Ok(Self::NoOp),
			other => Err(Error::Invalid(format!(
				"unknown Safety Stop outcome {other}"
			))),
		}
	}
}

/// A validated command. The Tenant and Run are named by the caller; the
/// expected control revision comes from a fresh read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
	pub tenant: String,
	pub run_id: Uuid,
	pub action: Action,
	pub expected_control_revision: i64,
	pub operation_id: Uuid,
	pub reason: String,
}

impl Command {
	pub fn new(
		tenant: &str,
		run_id: Uuid,
		action: Action,
		expected_control_revision: i64,
		operation_id: Uuid,
		reason: &str,
	) -> Result<Self> {
		if expected_control_revision < 0 {
			return Err(Error::Invalid(
				"the expected control revision cannot be negative".into(),
			));
		}
		Ok(Self {
			tenant: crate::tenancy::TenantName::new(tenant)?.into(),
			run_id,
			action,
			expected_control_revision,
			operation_id,
			reason: bounded_reason(reason, "a Safety Stop")?,
		})
	}

	/// A digest of everything but the operation ID. Reusing an operation ID with
	/// a different command conflicts instead of replaying.
	pub fn fingerprint(&self) -> String {
		let canonical = serde_json::json!({
			"tenant": self.tenant,
			"run_id": self.run_id,
			"action": self.action,
			"expected_control_revision": self.expected_control_revision,
			"reason": self.reason,
		});
		let digest = Sha256::digest(canonical.to_string().as_bytes());
		digest.iter().map(|byte| format!("{byte:02x}")).collect()
	}
}

/// One Safety Stop as the Tenant's audit readers see it (`decision.read`). The
/// reason is visible here and nowhere a Run's ordinary readers look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Record {
	pub id: Uuid,
	pub run_id: Uuid,
	pub action: Action,
	pub actor_kind: String,
	/// The browser Identity of a named administrator; absent for the shared token.
	pub identity_id: Option<Uuid>,
	pub reason: String,
	pub operation_id: Uuid,
	pub outcome: Outcome,
	pub created_at: chrono::DateTime<chrono::Utc>,
}

/// What the Run's current state makes of a new command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
	Accepted,
	NoOp,
	/// A control decision intervened; the caller must read again.
	Stale,
}

/// Decides a new command against the Run's state, after replay was ruled out.
///
/// A Run that has ended, or whose cancellation is already requested, changes
/// nothing whatever the revision. Otherwise the command needs the current
/// control revision. A pause of an already paused Run is accepted: control is
/// unchanged but its source and revision are not.
pub fn decide(
	phase: RunPhase,
	control: RunControl,
	control_revision: i64,
	command: &Command,
) -> Decision {
	if phase.is_terminal() || control == RunControl::Cancelled {
		return Decision::NoOp;
	}
	if command.expected_control_revision != control_revision {
		return Decision::Stale;
	}
	Decision::Accepted
}

#[cfg(test)]
mod tests;
