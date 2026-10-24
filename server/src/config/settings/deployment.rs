//! Strict public Deployment bootstrap descriptor, shared by every settings consumer.
use aidash_application::federation::key::DeploymentKeySource;
use aidash_domain::offering::Offering;
use reinhardt::conf::settings::sources::{ConfigSource, ScopedSource, SourceError};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
	deployment: Deployment,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
	offering: Offering,
	key: Option<Kms>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Kms {
	kms: String,
}

pub(super) struct ManagedSource(PathBuf);

impl ManagedSource {
	pub(super) fn new(path: impl Into<PathBuf>) -> Self {
		Self(path.into())
	}
}

impl ConfigSource for ManagedSource {
	fn load(&self) -> Result<indexmap::IndexMap<String, serde_json::Value>, SourceError> {
		let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&self.0)?)?;
		let descriptor: Descriptor = serde_json::from_value(value.clone())?;
		// Omission inherits profile settings; null must never silently disable a key.
		if value["deployment"]
			.get("key")
			.is_some_and(serde_json::Value::is_null)
		{
			return Err(SourceError::InvalidSource(
				"deployment.key cannot be null".into(),
			));
		}
		if let Some(key) = descriptor.deployment.key {
			if descriptor.deployment.offering == Offering::SelfHosted {
				return Err(SourceError::InvalidSource(
					"managed KMS Deployment Keys require an Enterprise Offering".into(),
				));
			}
			DeploymentKeySource::Kms(key.kms)
				.validate()
				.map_err(|error| SourceError::InvalidSource(error.to_string()))?;
		}
		Ok(value
			.as_object()
			.expect("validated descriptor object")
			.iter()
			.map(|(key, value)| (key.clone(), value.clone()))
			.collect())
	}

	fn load_scoped(&self) -> Result<ScopedSource, SourceError> {
		Ok(ScopedSource {
			values: self.load()?,
			interpolation_file: None,
		})
	}

	fn priority(&self) -> u8 {
		55
	}

	fn description(&self) -> String {
		"Managed Deployment settings".into()
	}
}
