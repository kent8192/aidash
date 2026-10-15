//! Retention Rules: how long Aidash keeps content it holds only for a while
//! (ADR 0045). The Deployment's periods are also the longest any Tenant may
//! keep; a Tenant may keep shorter ones. Every period is finite.
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The policy action that permits a subject to change its Tenant's periods,
/// evaluated on the Tenant's `retention` collection.
pub const UPDATE: &str = "retention.update";
/// The resource kind [`UPDATE`] is evaluated on.
pub const RESOURCE: &str = "retention";

/// Who decided a change of a Tenant's periods or an Evidence Revocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decider {
	/// A subject whose Tenant policy permits the action.
	Subject { tenant: String, subject: String },
	/// A Deployment Administrator, who needs no Tenant policy: the dashboard
	/// identity holding the grant, or none for the deployment's API token.
	DeploymentAdministrator {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		identity: Option<Uuid>,
	},
}

impl Decider {
	/// The form kept in history: `subject:<tenant>/<subject>`,
	/// `deployment-administrator:<identity>` or `deployment-administrator`.
	pub fn recorded(&self) -> String {
		match self {
			Self::Subject { tenant, subject } => format!("subject:{tenant}/{subject}"),
			Self::DeploymentAdministrator {
				identity: Some(identity),
			} => format!("deployment-administrator:{identity}"),
			Self::DeploymentAdministrator { identity: None } => "deployment-administrator".into(),
		}
	}
}

/// The longest period a rule may name: ten years.
pub const MAX_PERIOD_SECONDS: u32 = 3_650 * 86_400;

/// How long the contents of a Document snapshot are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPeriods {
	/// Kept this long after the snapshot's Run ends.
	pub after_run_seconds: u32,
	/// Never kept longer than this after retrieval, even while the Run lasts.
	pub max_age_seconds: u32,
}

impl SnapshotPeriods {
	/// Whether the contents still have a retention basis at `now`: the Run
	/// has not ended, or ended less than the post-run period ago, and the
	/// snapshot is younger than the maximum age.
	pub fn retains(
		&self,
		fetched_at: DateTime<Utc>,
		run_ended_at: Option<DateTime<Utc>>,
		now: DateTime<Utc>,
	) -> bool {
		let within = |since: DateTime<Utc>, seconds: u32| {
			now < since + Duration::seconds(i64::from(seconds))
		};
		within(fetched_at, self.max_age_seconds)
			&& run_ended_at.is_none_or(|ended| within(ended, self.after_run_seconds))
	}

	fn shorter(self, other: Self) -> Self {
		Self {
			after_run_seconds: self.after_run_seconds.min(other.after_run_seconds),
			max_age_seconds: self.max_age_seconds.min(other.max_age_seconds),
		}
	}

	fn within(&self, ceiling: &Self) -> bool {
		self.after_run_seconds <= ceiling.after_run_seconds
			&& self.max_age_seconds <= ceiling.max_age_seconds
	}
}

/// The periods of every kind of content Aidash keeps only for a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionRules {
	pub document_snapshots: SnapshotPeriods,
}

impl RetentionRules {
	/// The periods every Offering supplies: 24 hours after the Run ends, and
	/// at most 7 days after retrieval.
	pub const DEFAULT: Self = Self {
		document_snapshots: SnapshotPeriods {
			after_run_seconds: 86_400,
			max_age_seconds: 7 * 86_400,
		},
	};

	/// Every period is finite and at most [`MAX_PERIOD_SECONDS`].
	pub fn validate(&self) -> Result<()> {
		let periods = self.document_snapshots;
		if periods.after_run_seconds > MAX_PERIOD_SECONDS
			|| periods.max_age_seconds > MAX_PERIOD_SECONDS
		{
			return Err(Error::Invalid(
				"retention periods must not exceed ten years".into(),
			));
		}
		Ok(())
	}

	/// A Tenant's periods: valid, and none longer than the Deployment's.
	pub fn validate_for_tenant(&self, deployment: &Self) -> Result<()> {
		self.validate()?;
		if !self
			.document_snapshots
			.within(&deployment.document_snapshots)
		{
			return Err(Error::Invalid(
				"a Tenant's retention periods cannot exceed the Deployment's".into(),
			));
		}
		Ok(())
	}

	/// The periods that apply to one Tenant. Stored Tenant periods longer than
	/// the Deployment's current ones, after the Deployment shortened them, are
	/// cut to the Deployment's.
	pub fn effective(&self, tenant: Option<&Self>) -> Self {
		match tenant {
			None => *self,
			Some(tenant) => Self {
				document_snapshots: self.document_snapshots.shorter(tenant.document_snapshots),
			},
		}
	}
}

#[cfg(test)]
mod tests;
