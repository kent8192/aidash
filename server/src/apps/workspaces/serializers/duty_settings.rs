//! The Deployment's Duty scheduler pace (#557, #1011).
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

/// `[duties]`: how often each replica looks for Duties whose time trigger is
/// due. There is no count limit or minimum interval (#1012). A change takes
/// effect when the server restarts.
#[settings(fragment = true, section = "duties", validate = false)]
pub struct DutySettings {
	/// Seconds between two scheduler passes on each replica: 1 to 300.
	#[setting(default = "15")]
	pub poll_seconds: u64,
}

impl DutySettings {
	pub fn poll(&self) -> crate::Result<std::time::Duration> {
		if !(1..=300).contains(&self.poll_seconds) {
			return Err(crate::Error::Invalid(
				"duties.poll_seconds must be between 1 and 300".into(),
			));
		}
		Ok(std::time::Duration::from_secs(self.poll_seconds))
	}
}

impl SettingsValidation for DutySettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		self.poll()
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
