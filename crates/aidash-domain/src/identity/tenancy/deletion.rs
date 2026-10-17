//! Tenant Deletion (ADR 0060): a recorded decision that makes a Tenant unusable
//! at once and keeps it restorable for a grace period, after which its data is
//! purged (ADR 0061). These rules decide who may undo a deletion and when.
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};

/// The grace period unless the Deployment sets another.
pub const DEFAULT_GRACE_DAYS: u32 = 30;
/// The grace period is always finite, and never shorter than a day.
pub const MIN_GRACE_DAYS: u32 = 1;
pub const MAX_GRACE_DAYS: u32 = 90;

/// Who records or undoes a Tenant Deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
	/// A subject the Tenant's own policy allows `tenant.delete`, acting through
	/// its Mapping.
	TenantAdministrator,
	/// A Deployment Administrator, through the bearer token or a browser session.
	DeploymentAdministrator,
}

impl Actor {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::TenantAdministrator => "tenant_administrator",
			Self::DeploymentAdministrator => "deployment_administrator",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"tenant_administrator" => Ok(Self::TenantAdministrator),
			"deployment_administrator" => Ok(Self::DeploymentAdministrator),
			other => Err(Error::Invalid(format!("unknown deletion actor: {other}"))),
		}
	}
}

/// Checks a grace period setting.
pub fn grace_days(days: u32) -> Result<u32> {
	if !(MIN_GRACE_DAYS..=MAX_GRACE_DAYS).contains(&days) {
		return Err(Error::Invalid(format!(
			"the Tenant Deletion grace period lasts {MIN_GRACE_DAYS} to {MAX_GRACE_DAYS} days"
		)));
	}
	Ok(days)
}

/// When a deletion recorded at `deleted_at` stops being restorable.
pub fn purge_after(deleted_at: DateTime<Utc>, grace_days: u32) -> DateTime<Utc> {
	deleted_at + Duration::days(i64::from(grace_days))
}

/// The person deleting a Tenant types its Tenant name, which only those who
/// can see the Tenant know; its label may be shared by many Tenants.
pub fn confirm(tenant: &str, confirmation: &str) -> Result<()> {
	if confirmation.trim() != tenant {
		return Err(Error::Invalid(
			"type the Tenant name to confirm its deletion".into(),
		));
	}
	Ok(())
}

/// One recorded deletion, as restoring it needs to judge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deletion {
	pub deleted_by: Actor,
	pub purge_after: DateTime<Utc>,
	pub restored: bool,
	pub purged: bool,
}

/// Whether `restorer` may restore `deletion` at `now`. Only a Deployment
/// Administrator undoes a Deployment Administrator's deletion, so a Tenant
/// cannot reverse a Deployment-level measure.
pub fn check_restore(deletion: &Deletion, restorer: Actor, now: DateTime<Utc>) -> Result<()> {
	if deletion.restored {
		return Err(Error::Conflict("the Tenant is not deleted".into()));
	}
	if deletion.purged {
		return Err(Error::Conflict("tenant_purged".into()));
	}
	if now >= deletion.purge_after {
		return Err(Error::Conflict("tenant_grace_period_ended".into()));
	}
	if deletion.deleted_by == Actor::DeploymentAdministrator
		&& restorer != Actor::DeploymentAdministrator
	{
		return Err(Error::Conflict(
			"only a Deployment Administrator restores a Tenant a Deployment Administrator deleted"
				.into(),
		));
	}
	Ok(())
}

/// Whether `restorer` could restore `deletion` now, for offering the action.
pub fn restorable(deletion: &Deletion, restorer: Actor, now: DateTime<Utc>) -> bool {
	check_restore(deletion, restorer, now).is_ok()
}

#[cfg(test)]
mod tests;
