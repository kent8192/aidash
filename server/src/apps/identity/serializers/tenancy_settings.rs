//! The Deployment's Tenancy Rules. They live in server settings, like the
//! Retention Rules: whoever runs the Deployment changes them and restarts. The
//! Deployment's Offering supplies each rule left out here.
use aidash_domain::identity::tenancy::TenancyRules;
use aidash_domain::offering::Offering;
use reinhardt::{
	conf::settings::{
		fragment::SettingsValidation,
		profile::Profile,
		validation::{ValidationError, ValidationResult},
	},
	settings,
};

/// `[tenancy]`. Identities restricted to one Tenant, such as a Tenant Binding's,
/// are never covered: they hold no Personal Tenant, create no Tenants and redeem
/// no other Tenant's Invitation, whatever these say.
#[settings(fragment = true, section = "tenancy", validate = false)]
pub struct TenancySettings {
	/// Create a Personal Tenant for each External Identity at its first sign-in.
	#[setting(default = "None")]
	pub personal_tenants: Option<bool>,
	/// Let any signed-in External Identity create Tenants. Deployment
	/// Administrators can always create Tenants, without limits.
	#[setting(default = "None")]
	pub tenant_creation: Option<bool>,
	/// Creations per External Identity within any 60 minutes.
	#[setting(default = "None")]
	pub creation_per_hour: Option<u32>,
	/// Tenants one External Identity may have created and not yet purged.
	#[setting(default = "None")]
	pub creation_total: Option<u32>,
	/// Let Tenant Administrators and Deployment Administrators issue Invitations.
	#[setting(default = "None")]
	pub invitations: Option<bool>,
	/// Days a deleted Tenant stays restorable before its data is purged: 1 to 90.
	#[setting(default = "30")]
	pub deletion_grace_days: u32,
	/// Days a deleted Node stays restorable before purge: 1 to 90.
	#[setting(default = "30")]
	pub node_deletion_grace_days: u32,
	/// Days a purged Tenant's audit history is kept before it is deleted: 1 to 3650.
	#[setting(default = "365")]
	pub audit_retention_days: u32,
	/// How many Nodes a Tenant may hold: 1 to 1000. A Tier may be stricter.
	#[setting(default = "None")]
	pub nodes_per_tenant: Option<u32>,
}

impl TenancySettings {
	/// The rules of a Deployment that runs `offering`.
	pub fn rules(&self, offering: Offering) -> TenancyRules {
		let defaults = TenancyRules::of(offering);
		TenancyRules {
			personal_tenants: self.personal_tenants.unwrap_or(defaults.personal_tenants),
			tenant_creation: self.tenant_creation.unwrap_or(defaults.tenant_creation),
			creation_per_hour: self.creation_per_hour.unwrap_or(defaults.creation_per_hour),
			creation_total: self.creation_total.unwrap_or(defaults.creation_total),
			invitations: self.invitations.unwrap_or(defaults.invitations),
			deletion_grace_days: self.deletion_grace_days,
			node_deletion_grace_days: self.node_deletion_grace_days,
			audit_retention_days: self.audit_retention_days,
			nodes_per_tenant: self.nodes_per_tenant.unwrap_or(defaults.nodes_per_tenant),
		}
	}
}

impl SettingsValidation for TenancySettings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		if self.creation_per_hour.is_some_and(|limit| limit > 10_000)
			|| self.creation_total.is_some_and(|limit| limit > 100_000)
		{
			return Err(ValidationError::Constraint(
				"tenancy creation limits must be at most 10000 per hour and 100000 in total".into(),
			));
		}
		aidash_domain::identity::tenancy::deletion::grace_days(self.deletion_grace_days)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		aidash_domain::nodes::deletion::grace_days(self.node_deletion_grace_days)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		aidash_domain::identity::tenancy::deletion::audit_retention_days(self.audit_retention_days)
			.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		if let Some(limit) = self.nodes_per_tenant {
			aidash_domain::identity::tenancy::nodes_per_tenant(limit)
				.map_err(|error| ValidationError::Constraint(error.to_string()))?;
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::TenancySettings;
	use aidash_domain::offering::Offering;

	fn settings(value: serde_json::Value) -> TenancySettings {
		serde_json::from_value(value).unwrap()
	}

	#[test]
	fn an_explicit_rule_overrides_the_offering_default() {
		let cloud =
			settings(serde_json::json!({"personal_tenants": false, "deletion_grace_days": 30}))
				.rules(Offering::Cloud);
		assert!(!cloud.personal_tenants);
		assert!(cloud.tenant_creation);
		let self_hosted = settings(
			serde_json::json!({"tenant_creation": true, "creation_total": 3, "deletion_grace_days": 30}),
		)
		.rules(Offering::SelfHosted);
		assert!(!self_hosted.personal_tenants);
		assert!(self_hosted.tenant_creation);
		assert_eq!(self_hosted.creation_total, 3);
		assert_eq!(self_hosted.creation_per_hour, 5);
	}
}
