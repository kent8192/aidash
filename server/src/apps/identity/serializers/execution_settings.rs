//! A Tenant's execution settings (#1009).
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A Tenant that never stored settings reads the defaults at revision 0.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TenantExecutionSettings {
	/// Consecutive Lease Recoveries before a Crash Loop Failure: 1 to 20, 5 by
	/// default.
	pub lease_recovery_limit: u32,
	pub revision: i64,
	pub updated_at: Option<DateTime<Utc>>,
	pub updated_by: Option<String>,
}

/// Replaces the settings when `expected_revision` is still current (409
/// otherwise).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TenantExecutionSettingsUpdate {
	pub lease_recovery_limit: u32,
	pub expected_revision: i64,
}

impl From<crate::apps::identity::repositories::execution_settings::Stored>
	for TenantExecutionSettings
{
	fn from(stored: crate::apps::identity::repositories::execution_settings::Stored) -> Self {
		Self {
			lease_recovery_limit: stored.lease_recovery_limit,
			revision: stored.revision,
			updated_at: stored.updated_at,
			updated_by: stored.updated_by,
		}
	}
}
