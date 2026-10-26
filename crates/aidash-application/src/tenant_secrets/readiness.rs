//! Readiness facts for the Tenant Secrets a definition names (decision 5 of
//! #310). A fact says whether one bound item can carry its Tenant Secret now,
//! and who can repair it. It never carries a value, a Fingerprint or the last
//! four characters.
use super::Metadata;
use aidash_domain::tenant_secrets::permits;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether this Deployment offers Tenant Secrets at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "TenantSecretAvailability")]
pub enum Availability {
	/// No Secret Store can hold Tenant Secrets here.
	Unavailable,
	/// The Store seals hosts with the value but no Tenant Secret broker is
	/// configured, so a Cloud or Dedicated Deployment cannot attach it.
	BrokerNotConfigured,
	/// The Tenant's Secrets can be read directly or through the broker.
	Available,
}

/// What one bound item needs from the Tenant's Secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "TenantSecretReadinessStatus")]
pub enum Status {
	Ready,
	SecretMissing,
	SecretHostsMismatch,
	SecretsUnavailableOnDeployment,
	BrokerNotConfigured,
}

/// Who can repair a fact that is not ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "TenantSecretReadinessRepair")]
pub enum Repair {
	/// The caller holds `secret.manage` and can fix it in Tenant Secrets.
	ManageSecret,
	/// The caller cannot manage Secrets; someone who can must fix it.
	AskAdministrator,
	/// Only an operator of the Deployment can fix it.
	Operator,
	/// Nothing to repair.
	None,
}

/// Where a readiness fact belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "TenantSecretReadinessKind")]
pub enum Kind {
	Tool,
	ExternalService,
	/// One Secret slot a Marketplace package declares, before the installer
	/// chooses which of their Tenant Secrets fills it.
	Slot,
}

/// One item readiness fact. `item` names the bound tool or External service,
/// or the Marketplace package's Secret slot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TenantSecretReadinessFact")]
pub struct Fact {
	pub item: String,
	pub kind: Kind,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tenant_secret_name: Option<String>,
	pub status: Status,
	pub repair: Repair,
}

/// What one item names. A named Secret must exist and record the destination
/// host; a Marketplace slot may be filled by any of the Tenant's Secrets that
/// records the package's destination host.
#[derive(Debug, Clone)]
pub struct Target {
	pub item: String,
	pub kind: Kind,
	pub tenant_secret_name: Option<String>,
	pub endpoint: Option<String>,
}

fn repairable(status: Status, can_manage: bool) -> (Status, Repair) {
	(
		status,
		if can_manage {
			Repair::ManageSecret
		} else {
			Repair::AskAdministrator
		},
	)
}

fn named(
	name: &str,
	endpoint: Option<&str>,
	can_manage: bool,
	secrets: &[Metadata],
) -> (Status, Repair) {
	let Some(secret) = secrets.iter().find(|secret| secret.name == name) else {
		return repairable(Status::SecretMissing, can_manage);
	};
	let Some(endpoint) = endpoint else {
		return repairable(Status::SecretHostsMismatch, can_manage);
	};
	if permits(&secret.hosts, endpoint).unwrap_or(false) {
		(Status::Ready, Repair::None)
	} else {
		repairable(Status::SecretHostsMismatch, can_manage)
	}
}

fn slot(endpoint: Option<&str>, can_manage: bool, secrets: &[Metadata]) -> (Status, Repair) {
	let Some(endpoint) = endpoint else {
		return repairable(Status::SecretMissing, can_manage);
	};
	if secrets.is_empty() {
		return repairable(Status::SecretMissing, can_manage);
	}
	if secrets
		.iter()
		.any(|secret| permits(&secret.hosts, endpoint).unwrap_or(false))
	{
		(Status::Ready, Repair::None)
	} else {
		repairable(Status::SecretHostsMismatch, can_manage)
	}
}

fn judge(
	availability: Availability,
	can_manage: bool,
	target: &Target,
	secrets: &[Metadata],
) -> (Status, Repair) {
	match availability {
		Availability::Unavailable => (Status::SecretsUnavailableOnDeployment, Repair::Operator),
		Availability::BrokerNotConfigured => (Status::BrokerNotConfigured, Repair::Operator),
		Availability::Available => match target.kind {
			Kind::Slot => slot(target.endpoint.as_deref(), can_manage, secrets),
			Kind::Tool | Kind::ExternalService => match target.tenant_secret_name.as_deref() {
				// A tool that names no Tenant Secret needs none.
				None => (Status::Ready, Repair::None),
				Some(name) => named(name, target.endpoint.as_deref(), can_manage, secrets),
			},
		},
	}
}

/// The facts for `targets`, in their order. `secrets` are the Tenant's active
/// Tenant Secrets only.
pub fn facts(
	availability: Availability,
	can_manage: bool,
	targets: impl IntoIterator<Item = Target>,
	secrets: &[Metadata],
) -> Vec<Fact> {
	targets
		.into_iter()
		.map(|target| {
			let (status, repair) = judge(availability, can_manage, &target, secrets);
			Fact {
				item: target.item,
				kind: target.kind,
				tenant_secret_name: target.tenant_secret_name,
				status,
				repair,
			}
		})
		.collect()
}

#[cfg(test)]
mod tests;
