//! Recoverable Node deletion (ADR 0083). Grace is fixed at deletion; only live
//! Nodes consume capacity. Purge never removes the permanent name claim.
pub use crate::identity::tenancy::deletion::Actor;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};

pub const DEFAULT_GRACE_DAYS: u32 = 30;
pub const MIN_GRACE_DAYS: u32 = 1;
pub const MAX_GRACE_DAYS: u32 = 90;

pub fn grace_days(days: u32) -> Result<u32> {
	if !(MIN_GRACE_DAYS..=MAX_GRACE_DAYS).contains(&days) {
		return Err(Error::Invalid(
			"the Node Deletion grace period lasts 1 to 90 days".into(),
		));
	}
	Ok(days)
}

pub fn purge_after(deleted_at: DateTime<Utc>, days: u32) -> Result<DateTime<Utc>> {
	let days = grace_days(days)?;
	deleted_at
		.checked_add_signed(Duration::days(i64::from(days)))
		.ok_or_else(|| Error::Invalid("Node deletion deadline is out of range".into()))
}

/// The caller counts live Nodes while holding the Tenant's Node capacity lock.
pub fn check_delete(is_default: bool, live_count: u32) -> Result<()> {
	if is_default {
		return Err(Error::Conflict("node_is_default".into()));
	}
	if live_count <= 1 {
		return Err(Error::Conflict("node_is_last_live".into()));
	}
	Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deletion {
	pub deleted_by: Actor,
	pub purge_after: DateTime<Utc>,
	pub restored: bool,
	pub purged: bool,
}

/// Capacity and worker fencing are checked by the transaction owner as well.
pub fn check_restore(deletion: &Deletion, restorer: Actor, now: DateTime<Utc>) -> Result<()> {
	if deletion.restored {
		return Err(Error::Conflict("the Node is not deleted".into()));
	}
	if deletion.purged {
		return Err(Error::Conflict("node_purged".into()));
	}
	if now >= deletion.purge_after {
		return Err(Error::Conflict("node_grace_period_ended".into()));
	}
	if deletion.deleted_by == Actor::DeploymentAdministrator
		&& restorer != Actor::DeploymentAdministrator
	{
		return Err(Error::Conflict(
			"only a Deployment Administrator restores a Node a Deployment Administrator deleted"
				.into(),
		));
	}
	Ok(())
}

/// Each step is idempotent and resumes from its recorded position. Shared
/// Provider Credentials are not Node-owned and must survive a Node purge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PurgeStep {
	CapabilityObjects,
	Rows,
	Finish,
}
impl PurgeStep {
	pub const ALL: [Self; 3] = [Self::CapabilityObjects, Self::Rows, Self::Finish];
	pub const FIRST: Self = Self::CapabilityObjects;
	pub fn as_str(self) -> &'static str {
		match self {
			Self::CapabilityObjects => "capability_objects",
			Self::Rows => "rows",
			Self::Finish => "finish",
		}
	}
	pub fn parse(value: &str) -> Result<Self> {
		Self::ALL
			.into_iter()
			.find(|step| step.as_str() == value)
			.ok_or_else(|| Error::Invalid(format!("unknown Node purge step: {value}")))
	}
	pub fn next(self) -> Option<Self> {
		Self::ALL.get(self as usize + 1).copied()
	}
}
