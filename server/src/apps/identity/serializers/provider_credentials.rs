//! Provider Credential metadata management contracts; no public Key Material input.
use serde::{Deserialize, Serialize};
use uuid::Uuid;
mod settings;
pub(crate) use settings::ManagedSource;
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

/// References privately supplied keys outside the Registry secret namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySource {
	pub file: Option<std::path::PathBuf>,
	pub env: Option<String>,
}
impl KeySource {
	fn validate_shape(&self) -> std::result::Result<(), String> {
		match (&self.file, &self.env) {
			(Some(file), None) if !file.as_os_str().is_empty() => Ok(()),
			(None, Some(env))
				if !env.is_empty()
					&& !env.starts_with("AIDASH_SECRET_")
					// Skill imports forward these names to `gh`, which sends
					// GH_TOKEN/GITHUB_TOKEN to GitHub as authentication.
					&& !aidash_integrations::skill_import::gh_environment_allowed(
						std::ffi::OsStr::new(env),
					) && env.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
			{
				Ok(())
			}
			_ => Err(
				"key source must name exactly one nonempty file or env outside AIDASH_SECRET_* and the gh CLI environment"
					.into(),
			),
		}
	}
	pub async fn load(&self, label: &str) -> crate::Result<secrecy::SecretString> {
		use secrecy::ExposeSecret;
		self.validate_shape().map_err(crate::Error::Invalid)?;
		let value: secrecy::SecretString = match (&self.file, &self.env) {
			(Some(file), None) => tokio::fs::read_to_string(file)
				.await
				.map_err(|_| {
					crate::Error::Invalid(format!("{label} file is missing or unreadable"))
				})?
				.into(),
			(None, Some(env)) => std::env::var(env)
				.map_err(|_| {
					crate::Error::Invalid(format!(
						"{label} environment variable is missing or unreadable"
					))
				})?
				.into(),
			_ => unreachable!("validated key source"),
		};
		Ok(value.expose_secret().trim().to_owned().into())
	}
}
/// Where Key Material lives. Community provides the PostgreSQL Store; any
/// other kind, such as `secret_manager`, is an Enterprise build's, which reads
/// its fields (ADR 0032).
#[derive(Debug, Clone)]
pub enum StoreConfig {
	Postgres {
		master_key: KeySource,
		retired_master_keys: Vec<KeySource>,
	},
	Enterprise(serde_json::Map<String, serde_json::Value>),
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Postgres {
	Postgres {
		master_key: KeySource,
		#[serde(default)]
		retired_master_keys: Vec<KeySource>,
	},
}
impl<'de> Deserialize<'de> for StoreConfig {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		use serde::de::Error as _;
		let fields = serde_json::Map::deserialize(deserializer)?;
		match fields.get("kind").and_then(serde_json::Value::as_str) {
			Some("postgres") => {
				let Postgres::Postgres {
					master_key,
					retired_master_keys,
				} = serde_json::from_value(fields.into()).map_err(D::Error::custom)?;
				Ok(Self::Postgres {
					master_key,
					retired_master_keys,
				})
			}
			Some(_) => Ok(Self::Enterprise(fields)),
			None => Err(D::Error::custom("a Provider Credential Store needs a kind")),
		}
	}
}
impl Serialize for StoreConfig {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			Self::Postgres {
				master_key,
				retired_master_keys,
			} => Postgres::Postgres {
				master_key: master_key.clone(),
				retired_master_keys: retired_master_keys.clone(),
			}
			.serialize(serializer),
			Self::Enterprise(fields) => fields.serialize(serializer),
		}
	}
}
impl StoreConfig {
	pub(crate) async fn load_postgres_keys(
		&self,
	) -> crate::Result<(secrecy::SecretString, Vec<secrecy::SecretString>)> {
		let Self::Postgres {
			master_key,
			retired_master_keys,
		} = self
		else {
			return Err(crate::Error::Invalid(
				"Provider Credential Store recovery requires a PostgreSQL Store".into(),
			));
		};
		let current = master_key
			.load("Provider Credential Store Master Key")
			.await?;
		let mut retired = Vec::new();
		for source in retired_master_keys {
			retired.push(
				source
					.load("Provider Credential Store retired Master Key")
					.await?,
			);
		}
		Ok((current, retired))
	}
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
	#[setting(default = "20")]
	pub max_per_tenant: usize,
	#[setting(default = "None", leaf)]
	pub fingerprint_key: Option<KeySource>,
	#[setting(default = "None", leaf)]
	pub store: Option<StoreConfig>,
	/// The Credential Broker of an Enterprise Store, read by that build.
	#[setting(default = "None", leaf)]
	pub broker: Option<serde_json::Value>,
}
impl Settings {
	pub(crate) async fn load_fingerprint_key(&self) -> crate::Result<secrecy::SecretString> {
		use secrecy::ExposeSecret;
		self.validate(&Profile::parse("local"))
			.map_err(|e| crate::Error::Invalid(e.to_string()))?;
		let key = self
			.fingerprint_key
			.as_ref()
			.ok_or_else(|| {
				crate::Error::Invalid("Provider Credential Store requires fingerprint_key".into())
			})?
			.load("Provider Credential fingerprint key")
			.await?;
		if key.expose_secret().len() < 32 {
			return Err(crate::Error::Invalid(
				"Provider Credential fingerprint key must be at least 32 bytes".into(),
			));
		}
		Ok(key)
	}
}
impl SettingsValidation for Settings {
	fn validate(&self, _: &Profile) -> ValidationResult {
		let invalid = ValidationError::Constraint;
		if !(1..=1000).contains(&self.max_per_tenant) {
			return Err(invalid("Provider Credential quota must be 1..1000".into()));
		}
		if let Some(key) = &self.fingerprint_key {
			key.validate_shape().map_err(invalid)?;
		}
		if self.broker.is_some() && !matches!(self.store, Some(StoreConfig::Enterprise(_))) {
			return Err(invalid(
				"a Credential Broker serves only an Enterprise Provider Credential Store".into(),
			));
		}
		if let Some(store) = &self.store {
			if self.fingerprint_key.is_none() {
				return Err(invalid(
					"Provider Credential Store requires fingerprint_key".into(),
				));
			}
			match store {
				StoreConfig::Postgres {
					master_key,
					retired_master_keys,
				} => {
					master_key.validate_shape().map_err(invalid)?;
					for key in retired_master_keys {
						key.validate_shape().map_err(invalid)?;
					}
				}
				StoreConfig::Enterprise(_) => {}
			}
		}
		Ok(())
	}
}
