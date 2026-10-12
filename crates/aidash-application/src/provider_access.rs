//! Per-call provider access. Key Material never crosses Registry or federation metadata.
use crate::{Error, Result};
use async_trait::async_trait;
use secrecy::SecretString;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct Context {
	pub tenant: String,
	pub run: Option<Uuid>,
	/// Local maintenance authority when no Run exists; used by #137's Token Subject.
	pub maintenance: Option<MaintenancePurpose>,
	/// Admission pins an ID, not a Secret Manager version or a mutable binding.
	pub provider_credential_id: Option<Uuid>,
	/// Approved per-invocation model and bounds; populated by the inference adapter.
	pub inference: Option<Inference>,
}
#[derive(Debug, Clone)]
pub struct Inference {
	pub model: String,
	pub operations: Vec<Operation>,
	pub max_output_tokens: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
	Chat,
	Discovery,
	Embeddings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenancePurpose {
	MemoryIndexing,
	MemoryRetention,
	MemoryReflection,
	MemoryRetrieval,
}
/// Where a model-provider call gets its bearer. There is no environment source:
/// model-provider keys come only from Tenant Provider Credentials (ADR 0006).
#[derive(Debug, Clone)]
pub enum Source {
	/// The configured endpoint is called without a key.
	Keyless,
	/// The Tenant's Provider Credential for this Provider Catalog entry.
	Tenant { provider: String },
}
impl Source {
	pub fn configured(provider: &Option<String>) -> Self {
		match provider {
			Some(provider) => Self::Tenant {
				provider: provider.clone(),
			},
			None => Self::Keyless,
		}
	}
}
#[derive(Debug)]
pub struct Access {
	pub endpoint: String,
	pub bearer: SecretString,
}
#[async_trait]
pub trait ProviderAccess: Send + Sync {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access>;
}
/// Access without a Provider Credential Store: keyless endpoints only.
pub struct KeylessAccess;
#[async_trait]
impl ProviderAccess for KeylessAccess {
	async fn resolve(&self, _: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		let Source::Keyless = source else {
			return Err(Error::Invalid(
				"Provider Credential Store is not configured".into(),
			));
		};
		aidash_domain::configuration::validate_endpoint(endpoint)?;
		Ok(Access {
			endpoint: endpoint.into(),
			bearer: String::new().into(),
		})
	}
}

/// Reads only one explicitly pinned version of Key Material.
#[async_trait]
pub trait KeyMaterialReader: Send + Sync {
	async fn read(&self, tenant: &str, resource: &str, version: &str) -> Result<SecretString>;
}

/// Worker-local minting port. It receives validated metadata, never Key Material.
#[async_trait]
pub trait TokenIssuer: Send + Sync {
	async fn mint(
		&self,
		context: &Context,
		credential: &aidash_domain::provider_credentials::ProviderCredential,
	) -> Result<Access>;
}
/// Calls reload current metadata for the admitted ID before each mint or pinned read.
/// Cloud mints through the Credential Broker; self-hosted PostgreSQL Stores read
/// the pin locally. Without either, Tenant access fails closed.
pub struct TenantAccess {
	pub keyless: KeylessAccess,
	pub repository: Arc<dyn crate::provider_credentials::Repository>,
	pub reader: Option<Arc<dyn KeyMaterialReader>>,
	pub issuer: Option<Arc<dyn TokenIssuer>>,
}
#[async_trait]
impl ProviderAccess for TenantAccess {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		let Source::Tenant { provider } = source else {
			return self.keyless.resolve(context, endpoint, source).await;
		};
		let provider = aidash_domain::provider_credentials::Provider::parse(provider)?;
		if context.tenant.is_empty()
			|| (context.run.is_none() && context.maintenance.is_none())
			|| (context.run.is_some() && context.maintenance.is_some())
			|| endpoint != provider.base_url()
		{
			return Err(Error::Invalid(
				"Provider Credential requires a local Tenant and Run or maintenance purpose".into(),
			));
		}
		let mut scope = self.repository.begin(&context.tenant).await?;
		let id = if context.run.is_some() {
			context.provider_credential_id.ok_or_else(|| {
				Error::Invalid("Provider Credential access has no resolved ID".into())
			})?
		} else {
			scope
				.bindings()
				.await?
				.into_iter()
				.find(|binding| binding.tenant == context.tenant && binding.provider == provider)
				.and_then(|binding| binding.provider_credential_id)
				.ok_or_else(|| Error::Invalid("Provider Credential binding is missing".into()))?
		};
		let row = scope.get(id).await?;
		if row.tenant != context.tenant
			|| row.provider != provider
			|| row.base_url != provider.base_url()
		{
			return Err(Error::Invalid(
				"Provider Credential does not match the admitted Tenant and provider".into(),
			));
		}
		// Reload the current pin on every call: rotation takes effect immediately;
		// a later binding change cannot alter the ID admitted for this Run.
		let version = row.require_active()?.to_owned();
		scope.commit().await?;
		if let Some(issuer) = &self.issuer {
			return issuer.mint(context, &row).await;
		}
		let reader = self
			.reader
			.as_ref()
			.ok_or_else(|| Error::Invalid("credential broker not configured".into()))?;
		let bearer = reader
			.read(&context.tenant, &row.secret_resource, &version)
			.await?;
		Ok(Access {
			endpoint: provider.base_url().into(),
			bearer,
		})
	}
}
