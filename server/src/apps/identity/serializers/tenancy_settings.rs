//! The Deployment's Tenancy Rules. They live in server settings, like the
//! Retention Rules: whoever runs the Deployment changes them and restarts. The
//! defaults are the Self-hosted Offering's until the Offering supplies them.
use aidash_domain::identity::tenancy::TenancyRules;
use reinhardt::{
	conf::settings::{
		fragment::SettingsValidation,
		profile::Profile,
		validation::{ValidationError, ValidationResult},
	},
	settings,
};

/// `[tenancy]`. Identities of a GCIP Tenant with a Tenant Binding are never
/// covered: they hold no Personal Tenant, create no Tenants and redeem no other
/// Tenant's Invitation, whatever these say.
#[settings(fragment = true, section = "tenancy", validate = false)]
pub struct TenancySettings {
	/// Create a Personal Tenant for each External Identity at its first sign-in.
	#[setting(default = "false")]
	pub personal_tenants: bool,
	/// Let any signed-in External Identity create Tenants. Deployment
	/// Administrators can always create Tenants, without limits.
	#[setting(default = "false")]
	pub tenant_creation: bool,
	/// Creations per External Identity within any 60 minutes.
	#[setting(default = "5")]
	pub creation_per_hour: u32,
	/// Tenants one External Identity may have created and not yet purged.
	#[setting(default = "20")]
	pub creation_total: u32,
	/// Let Tenant Administrators and Deployment Administrators issue Invitations.
	#[setting(default = "true")]
	pub invitations: bool,
}

impl TenancySettings {
	pub fn rules(&self) -> TenancyRules {
		TenancyRules {
			personal_tenants: self.personal_tenants,
			tenant_creation: self.tenant_creation,
			creation_per_hour: self.creation_per_hour,
			creation_total: self.creation_total,
			invitations: self.invitations,
		}
	}
}

impl SettingsValidation for TenancySettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		if self.creation_per_hour > 10_000 || self.creation_total > 100_000 {
			return Err(ValidationError::Constraint(
				"tenancy creation limits must be at most 10000 per hour and 100000 in total".into(),
			));
		}
		Ok(())
	}
}
