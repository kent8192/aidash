//! The Deployment's Retention Rules for Research evidence (ADR 0045).
use aidash_domain::retention::{MAX_PERIOD_SECONDS, RetentionRules, SnapshotPeriods};
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

/// `[research_retention]` with `snapshot_after_run_seconds` and
/// `snapshot_max_age_seconds`: the code defaults, which are the same in every
/// Offering, unless whoever runs the Deployment sets other finite periods. A
/// change takes effect when the server restarts.
#[settings(fragment = true, section = "research_retention", validate = false)]
pub struct ResearchRetentionSettings {
	#[setting(default = "None")]
	pub snapshot_after_run_seconds: Option<u32>,
	#[setting(default = "None")]
	pub snapshot_max_age_seconds: Option<u32>,
}

impl ResearchRetentionSettings {
	/// The Deployment's Retention Rules; a period over ten years is refused.
	pub fn rules(&self) -> crate::Result<RetentionRules> {
		let period = |name: &str, value: Option<u32>, default: u32| {
			let value = value.unwrap_or(default);
			if value > MAX_PERIOD_SECONDS {
				return Err(crate::Error::Invalid(format!(
					"research_retention.{name} must not exceed ten years ({MAX_PERIOD_SECONDS} seconds)"
				)));
			}
			Ok(value)
		};
		let defaults = RetentionRules::DEFAULT.document_snapshots;
		Ok(RetentionRules {
			document_snapshots: SnapshotPeriods {
				after_run_seconds: period(
					"snapshot_after_run_seconds",
					self.snapshot_after_run_seconds,
					defaults.after_run_seconds,
				)?,
				max_age_seconds: period(
					"snapshot_max_age_seconds",
					self.snapshot_max_age_seconds,
					defaults.max_age_seconds,
				)?,
			},
		})
	}
}

impl SettingsValidation for ResearchRetentionSettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		self.rules()
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
