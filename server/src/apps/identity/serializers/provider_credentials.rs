//! Provider Credential metadata management contracts; no public Key Material input.
mod settings;
use serde::{Deserialize, Serialize};
pub(crate) use settings::ManagedSource;
use uuid::Uuid;
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ProviderCredentialRevision")]
pub struct Revision {
	pub expected_revision: i64,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ProviderCredentialBindingUpdate")]
pub struct BindingUpdate {
	pub expected_revision: i64,
	#[serde(deserialize_with = "binding_target")]
	#[schemars(required, schema_with = "binding_target_schema")]
	pub provider_credential_id: Option<Uuid>,
}
// Presence is required, while null is the explicit unbind operation. Schemars'
// `required` alone unwraps Option and would incorrectly remove null from the SDK.
fn binding_target_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
	<Option<Uuid> as schemars::JsonSchema>::json_schema(generator)
}
fn binding_target<'de, D: serde::Deserializer<'de>>(
	deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
	Option::<Uuid>::deserialize(deserializer)
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ProviderCredentialPage")]
pub struct Page {
	#[serde(default)]
	pub offset: usize,
	#[serde(default = "page_limit")]
	pub limit: usize,
}
fn page_limit() -> usize {
	50
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreConfig {
	pub byok_project_id: String,
	pub environment_id: String,
	pub fingerprint_env: String,
	#[serde(default = "tenant_limit")]
	pub max_per_tenant: usize,
}
fn tenant_limit() -> usize {
	20
}
use reinhardt::{
	conf::settings::{
		fragment::SettingsValidation,
		profile::Profile,
		validation::{ValidationError, ValidationResult},
	},
	settings,
};
#[settings(fragment = true, section = "provider_credentials", validate = false)]
pub struct Settings {
	#[setting(default = "None", leaf)]
	pub store: Option<StoreConfig>,
}
impl SettingsValidation for Settings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		if let Some(store) = &self.store {
			aidash_domain::configuration::validate_secret_reference(&store.fingerprint_env)
				.map_err(|e| ValidationError::Constraint(e.to_string()))?;
			aidash_integrations::provider_credentials::SecretManager::new(
				store.byok_project_id.clone(),
				store.environment_id.clone(),
			)
			.map_err(|e| ValidationError::Constraint(e.to_string()))?;
			if !(1..=1000).contains(&store.max_per_tenant) {
				return Err(ValidationError::Constraint(
					"Provider Credential quota must be 1..1000".into(),
				));
			}
		}
		Ok(())
	}
}
