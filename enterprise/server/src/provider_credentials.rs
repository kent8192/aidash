// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! The Secret Manager Secret Store and its Credential Broker.
use aidash_gcp::capability::{
	KmsTokenSigner, MetadataTokenSource,
	issuer::{CapabilityIssuer, WorkerConfiguration},
};
use aidash_gcp::secret_manager::SecretManager;
use aidash_server::bootstrap::build::ProvidedStore;
use aidash_server::{Error, Result};
use serde::Deserialize;
use std::sync::Arc;

/// `[provider_credentials.store]` of `kind = "secret_manager"`.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreConfig {
	SecretManager {
		byok_project_id: String,
		environment_id: String,
	},
}

impl StoreConfig {
	fn validate(&self) -> Result<()> {
		let Self::SecretManager {
			byok_project_id,
			environment_id,
		} = self;
		if byok_project_id.is_empty()
			|| !byok_project_id
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
			|| environment_id.is_empty()
			|| !environment_id
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		{
			return Err(Error::Invalid(
				"invalid Secret Store project or environment".into(),
			));
		}
		Ok(())
	}
}

pub fn store(
	store: &serde_json::Map<String, serde_json::Value>,
	broker: Option<&serde_json::Value>,
) -> Result<ProvidedStore> {
	let kind = store
		.get("kind")
		.and_then(serde_json::Value::as_str)
		.unwrap_or_default();
	if kind != "secret_manager" {
		return Err(Error::Invalid(format!(
			"the Enterprise build has no {kind} Secret Store"
		)));
	}
	let config: StoreConfig = serde_json::from_value(store.clone().into())
		.map_err(|error| Error::Invalid(format!("invalid Secret Store: {error}")))?;
	config.validate()?;
	let broker = broker
		.map(|broker| {
			serde_json::from_value::<WorkerConfiguration>(broker.clone())
				.map_err(|error| Error::Invalid(format!("invalid Credential Broker: {error}")))
		})
		.transpose()?;
	let StoreConfig::SecretManager {
		byok_project_id,
		environment_id,
	} = config;
	let issuer =
		broker
			.map(|broker| -> Result<_> {
				broker.validate()?;
				// Only Cloud Secret Manager Stores are served by a Credential Broker.
				if broker.audience != environment_id {
					return Err(Error::Invalid(
					"Credential Broker audience must match the configured Secret Store environment"
						.into(),
				));
				}
				let tokens = Arc::new(MetadataTokenSource::new().map_err(|_| {
					Error::Invalid("Capability Token metadata configuration unavailable".into())
				})?);
				let signer = Arc::new(KmsTokenSigner::new(broker.kid.clone(), tokens).map_err(
					|_| Error::Invalid("invalid Capability Token KMS key version".into()),
				)?);
				Ok(Arc::new(CapabilityIssuer::new(
					broker,
					byok_project_id.clone(),
					signer,
				)?)
					as Arc<
						dyn aidash_application::provider_access::TokenIssuer,
					>)
			})
			.transpose()?;
	Ok(ProvidedStore {
		store: Arc::new(SecretManager::new(byok_project_id, environment_id)?),
		issuer,
	})
}

#[cfg(test)]
mod tests;
