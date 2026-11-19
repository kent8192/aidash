//! The Deployment's Schedule ceilings and scheduler pace (#557).
use aidash_domain::schedules::Ceilings;
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

/// `[schedules]`: the most Schedules any Tenant and any person may hold, the
/// shortest interval between two fire times, which a Tenant may only raise,
/// and how often each replica looks for due Schedules. A change takes effect
/// when the server restarts; lowering a ceiling also lowers what Tenants set.
#[settings(fragment = true, section = "schedules", validate = false)]
pub struct ScheduleSettings {
	#[setting(default = "1000")]
	pub max_schedules: i32,
	#[setting(default = "100")]
	pub max_per_person: i32,
	#[setting(default = "900")]
	pub min_interval_seconds: i64,
	/// Seconds between two scheduler passes on each replica: 1 to 300.
	#[setting(default = "15")]
	pub poll_seconds: u64,
}

impl ScheduleSettings {
	pub fn ceilings(&self) -> crate::Result<Ceilings> {
		let ceilings = Ceilings {
			max_schedules: self.max_schedules,
			max_per_person: self.max_per_person,
			min_interval_seconds: self.min_interval_seconds,
		};
		ceilings.validate()?;
		Ok(ceilings)
	}

	pub fn poll(&self) -> crate::Result<std::time::Duration> {
		if !(1..=300).contains(&self.poll_seconds) {
			return Err(crate::Error::Invalid(
				"schedules.poll_seconds must be between 1 and 300".into(),
			));
		}
		Ok(std::time::Duration::from_secs(self.poll_seconds))
	}
}

impl SettingsValidation for ScheduleSettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		self.ceilings()
			.and_then(|_| self.poll())
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
