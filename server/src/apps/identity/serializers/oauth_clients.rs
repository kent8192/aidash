//! OAuth management request bodies. Credentials are accepted only on writes.
use aidash_application::tenant_secrets::Ownership;
use secrecy::SecretString;
use serde::Deserialize;
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "OAuthClientWrite")]
pub struct Write {
	#[serde(default)]
	pub owner: Ownership,
	pub issuer: String,
	pub client_id: String,
	#[serde(default)]
	#[schemars(with = "Option<String>")]
	pub client_secret: Option<SecretString>,
	#[serde(default)]
	pub expected_revision: i64,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ConnectionScopeCeilingWrite")]
pub struct CeilingWrite {
	pub scopes: Vec<String>,
	#[serde(default)]
	pub expected_revision: i64,
}
