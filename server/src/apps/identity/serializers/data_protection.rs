//! Tenant Data Protection settings and a conservative impact preview.
use aidash_domain::data_protection::{DataProtection, Level};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TenantDataProtectionSettings")]
pub struct Settings {
	pub revision: u64,
	pub policy: DataProtection,
	pub effective: DataProtection,
	pub deployment_floor: Level,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "DataProtectionImpact")]
pub struct Impact {
	pub effective: DataProtection,
	pub agents: Vec<Affected>,
	pub catalog_approvals: Vec<Affected>,
	pub destinations: Vec<Affected>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "DataProtectionAffected")]
pub struct Affected {
	pub id: String,
	pub version: Option<String>,
	pub reason: String,
}

/// Existing clients may still replace the whole policy without a revision.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(rename = "TenantDataProtectionUpdate")]
pub struct Update {
	#[serde(default)]
	pub expected_revision: Option<u64>,
	#[serde(flatten)]
	pub policy: DataProtection,
}
