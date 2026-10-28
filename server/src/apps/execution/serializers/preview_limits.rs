//! `[preview_limits]`: the Deployment's ceilings on Live preview lifetimes.
//!
//! A Tenant may only lower these, as with Test Limits. A change takes effect
//! when the server restarts; a start reads both values.
use aidash_domain::preview::live::Limits;
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

#[settings(fragment = true, section = "preview_limits", validate = false)]
pub struct PreviewLimitSettings {
	/// The longest a Live preview may sit without a gateway request.
	#[setting(default = "900")]
	pub idle_seconds: i64,
	/// The longest a Live preview may run from its start.
	#[setting(default = "7200")]
	pub absolute_seconds: i64,
}

impl PreviewLimitSettings {
	pub fn limits(&self) -> crate::Result<Limits> {
		Limits::new(self.idle_seconds, self.absolute_seconds).map_err(crate::Error::from)
	}
}

impl SettingsValidation for PreviewLimitSettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		self.limits()
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
