//! Page reading as this Deployment offers it (#987). It is a Built-in
//! definition every Tenant may approve; a change takes effect when the server
//! restarts. Brave and the fixture provider are gone.
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

/// `[research_providers]` with `page_reading = true` to read pages with
/// Aidash's own page reader.
#[settings(fragment = true, section = "research_providers", validate = false)]
pub struct ResearchProviderSettings {
	#[setting(default = "false")]
	pub page_reading: bool,
}

impl ResearchProviderSettings {
	/// Nothing to refuse: page reading needs no secret or terms.
	pub fn check(&self, _profile: &Profile) -> crate::Result<()> {
		Ok(())
	}
}

impl SettingsValidation for ResearchProviderSettings {
	fn validate(&self, profile: &Profile) -> ValidationResult {
		self.check(profile)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
