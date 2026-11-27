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
	pub provider: Option<aidash_domain::oauth_connections::OAuthProvider>,
	/// Canonical issuer. Existing registrations cannot change issuer (409
	/// oauth_client_issuer_immutable); create a new registration to retarget.
	pub issuer: String,
	pub client_id: Option<String>,
	/// For a Configurator call, the answered text Setting Request with field client_id.
	pub client_id_request: Option<uuid::Uuid>,
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
