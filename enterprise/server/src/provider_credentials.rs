//! The Secret Manager Secret Store and its Credential Broker.
use aidash_gcp::capability::{
	KmsTokenSigner, MetadataTokenSource,
	issuer::{CapabilityIssuer, WorkerConfiguration},
	secret_calls::SecretCallsAdapter,
	secret_issuer::SecretIssuerConfiguration,
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

/// `tenant_secret_broker` from the managed settings: the server's Tenant
/// Secret broker endpoint, issuer, audience and server signing key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantSecretBrokerConfiguration {
	pub endpoint: String,
	pub issuer: String,
	pub audience: String,
	pub kid: String,
}

/// The Tenant Secret call adapter, or `None` when no broker is configured.
/// The server role signs with the broker's own key; the worker role reuses its
/// existing worker broker key and takes only the endpoint and audience here
/// (ADR 0095).
pub fn secret_calls(
	broker: Option<&serde_json::Value>,
	tenant_secret_broker: Option<&serde_json::Value>,
	worker: bool,
) -> Result<Option<Arc<dyn aidash_application::tenant_secrets::SecretCalls>>> {
	let Some((endpoint, configuration)) = call_configuration(broker, tenant_secret_broker, worker)?
	else {
		return Ok(None);
	};
	Ok(Some(Arc::new(SecretCallsAdapter::new(
		&endpoint,
		configuration,
	)?)))
}

pub fn connection_calls(
	broker: Option<&serde_json::Value>,
	tenant_secret_broker: Option<&serde_json::Value>,
	worker: bool,
) -> Result<Option<Arc<dyn aidash_application::connection_calls::ConnectionCalls>>> {
	let Some((endpoint, configuration)) = call_configuration(broker, tenant_secret_broker, worker)?
	else {
		return Ok(None);
	};
	Ok(Some(Arc::new(
		aidash_gcp::capability::connections::ConnectionCallsAdapter::new(&endpoint, configuration)?,
	)))
}

fn call_configuration(
	broker: Option<&serde_json::Value>,
	tenant_secret_broker: Option<&serde_json::Value>,
	worker: bool,
) -> Result<Option<(String, SecretIssuerConfiguration)>> {
	let Some(tenant_secret_broker) = tenant_secret_broker else {
		return Ok(None);
	};
	let tenant: TenantSecretBrokerConfiguration =
		serde_json::from_value(tenant_secret_broker.clone())
			.map_err(|error| Error::Invalid(format!("invalid Tenant Secret Broker: {error}")))?;
	let (issuer, kid) = if worker {
		let broker = broker.ok_or_else(|| {
			Error::Invalid(
				"the worker needs its Credential Broker key to sign Tenant Secret calls".into(),
			)
		})?;
		let broker: WorkerConfiguration = serde_json::from_value(broker.clone())
			.map_err(|error| Error::Invalid(format!("invalid Credential Broker: {error}")))?;
		broker.validate()?;
		(broker.issuer, broker.kid)
	} else {
		(tenant.issuer, tenant.kid)
	};
	let configuration = SecretIssuerConfiguration {
		issuer,
		audience: tenant.audience,
		kid,
	};
	configuration.validate()?;
	Ok(Some((tenant.endpoint, configuration)))
}

#[cfg(test)]
mod tests;
