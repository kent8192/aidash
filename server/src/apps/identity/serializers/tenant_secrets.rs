//! Tenant Secret management contracts. The value is accepted on write and is
//! never part of any response (ADR 0051).
use aidash_application::tenant_secrets::Ownership;
use secrecy::SecretString;
use serde::Deserialize;

/// Stores a new Tenant Secret (`expected_revision` 0, `value` and `hosts`
/// required), replaces its value, changes its hosts, or both. Replacing the
/// value changes no definition version.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "TenantSecretWrite")]
pub struct Write {
	pub expected_revision: i64,
	#[serde(default)]
	pub owner: Ownership,
	/// Typed or pasted by the Tenant; never returned.
	#[serde(default)]
	#[schemars(with = "Option<String>")]
	pub value: Option<SecretString>,
	/// The hosts the value may be sent to, such as `api.example.com`.
	#[serde(default)]
	pub hosts: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "TenantSecretRevision")]
pub struct Revision {
	pub expected_revision: i64,
	#[serde(default)]
	pub owner: Ownership,
}

/// Namespace selector for metadata reads; only the caller's personal namespace
/// can be addressed.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "TenantSecretOwnerQuery")]
pub struct OwnerQuery {
	#[serde(default)]
	pub owner: Ownership,
}
