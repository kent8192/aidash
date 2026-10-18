//! The Deployment's ceilings on every Tenant's Workbench Test Limits (#361).
use aidash_domain::registry::workbench::sandbox::TestLimitCeilings;
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;

/// `[workbench_test_limits]`: the largest Test Limits any Tenant may set.
/// The defaults are the largest values allowed at all, so a Deployment that
/// sets nothing leaves its Tenants the full range. Lowering a ceiling also
/// lowers values Tenants already set, from the next test on. A change takes
/// effect when the server restarts.
#[settings(fragment = true, section = "workbench_test_limits", validate = false)]
pub struct WorkbenchTestLimitSettings {
	#[setting(default = "1000000")]
	pub max_input_bytes: i32,
	#[setting(default = "8192")]
	pub max_output_tokens: i32,
	#[setting(default = "1000000")]
	pub max_total_tokens: i32,
	#[setting(default = "64")]
	pub max_steps: i32,
	#[setting(default = "900")]
	pub max_duration_secs: i32,
	#[setting(default = "32")]
	pub max_concurrent: i32,
	#[setting(default = "365")]
	pub payload_days: i32,
	#[setting(default = "3650")]
	pub incident_evidence_days: i32,
}

impl WorkbenchTestLimitSettings {
	pub fn ceilings(&self) -> crate::Result<TestLimitCeilings> {
		let ceilings = TestLimitCeilings {
			max_input_bytes: self.max_input_bytes,
			max_output_tokens: self.max_output_tokens,
			max_total_tokens: self.max_total_tokens,
			max_steps: self.max_steps,
			max_duration_secs: self.max_duration_secs,
			max_concurrent: self.max_concurrent,
			payload_days: self.payload_days,
			incident_evidence_days: self.incident_evidence_days,
		};
		ceilings.validate()?;
		Ok(ceilings)
	}
}

impl SettingsValidation for WorkbenchTestLimitSettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		self.ceilings()
			.map(drop)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}
