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

/// How long audit history outlives a purge unless the Deployment sets another.
pub const DEFAULT_AUDIT_RETENTION_DAYS: u32 = 365;
/// Audit history is always kept for a finite period.
pub const MIN_AUDIT_RETENTION_DAYS: u32 = 1;
pub const MAX_AUDIT_RETENTION_DAYS: u32 = 3_650;
/// Consecutive failures after which a purge is shown to Deployment
/// Administrators as stalled. The purge retries hourly.
pub const STALLED_AFTER_FAILURES: u32 = 3;

/// Checks a grace period setting.
pub fn grace_days(days: u32) -> Result<u32> {
	if !(MIN_GRACE_DAYS..=MAX_GRACE_DAYS).contains(&days) {
		return Err(Error::Invalid(format!(
			"the Tenant Deletion grace period lasts {MIN_GRACE_DAYS} to {MAX_GRACE_DAYS} days"
		)));
	}
	Ok(days)
}

/// Checks the setting for how long audit history outlives a purge.
pub fn audit_retention_days(days: u32) -> Result<u32> {
	if !(MIN_AUDIT_RETENTION_DAYS..=MAX_AUDIT_RETENTION_DAYS).contains(&days) {
		return Err(Error::Invalid(format!(
			"audit history outlives a purge for {MIN_AUDIT_RETENTION_DAYS} to {MAX_AUDIT_RETENTION_DAYS} days"
		)));
	}
	Ok(days)
}

/// When audit history kept at `purged_at` is deleted.
pub fn audit_erase_after(purged_at: DateTime<Utc>, retention_days: u32) -> DateTime<Utc> {
	purged_at + Duration::days(i64::from(retention_days))
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

/// The steps of a purge (ADR 0061), in the order they run. A step that fails is
/// retried from itself, so each one is idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PurgeStep {
	/// Revoke and delete the Provider Credentials, which ends their stored Key
	/// Material outside the database.
	ProviderCredentials,
	/// Delete the capability object files, which only `core_objects` names.
	CapabilityObjects,
	/// Delete every Tenant-owned row that is not audit history.
	Rows,
	/// Clear the root record's label and mark the deletion purged.
	Finish,
}

impl PurgeStep {
	pub const ALL: [Self; 4] = [
		Self::ProviderCredentials,
		Self::CapabilityObjects,
		Self::Rows,
		Self::Finish,
	];

	/// The step a purge that has not run yet starts with.
	pub const FIRST: Self = Self::ProviderCredentials;

	pub fn as_str(self) -> &'static str {
		match self {
			Self::ProviderCredentials => "provider_credentials",
			Self::CapabilityObjects => "capability_objects",
			Self::Rows => "rows",
			Self::Finish => "finish",
		}
	}

	pub fn parse(value: &str) -> Result<Self> {
		Self::ALL
			.into_iter()
			.find(|step| step.as_str() == value)
			.ok_or_else(|| Error::Invalid(format!("unknown purge step: {value}")))
	}

	/// The step after this one; none after the last.
	pub fn next(self) -> Option<Self> {
		Self::ALL.get(self as usize + 1).copied()
	}
}

/// Whether a purge that failed `failures` times in a row has stalled.
pub fn stalled(failures: u32) -> bool {
	failures >= STALLED_AFTER_FAILURES
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
