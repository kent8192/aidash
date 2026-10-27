//! Tenant Data Protection settings and a conservative impact preview.
use aidash_domain::data_protection::{DataProtection, Level};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "TenantDataProtectionSettings")]
pub struct Settings {
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
