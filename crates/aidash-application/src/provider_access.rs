//! Per-call provider access. Key Material never crosses Registry or federation metadata.
use crate::{Error, Result, ports::Credentials};
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
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenancePurpose {
	MemoryIndexing,
	MemoryRetention,
	MemoryReflection,
	MemoryRetrieval,
}
#[derive(Debug, Clone)]
pub enum Source {
	Env { reference: Option<String> },
	Tenant { provider: String },
}
impl Source {
	pub fn configured(env: &Option<String>, provider: &Option<String>) -> Self {
		match provider {
			Some(provider) => Self::Tenant {
				provider: provider.clone(),
			},
			None => Self::Env {
				reference: env.clone(),
			},
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
/// Self-hosted access retains per-invocation environment resolution and optional authentication.
pub struct EnvironmentAccess {
	pub credentials: Arc<dyn Credentials>,
}
#[async_trait]
impl ProviderAccess for EnvironmentAccess {
	async fn resolve(&self, _: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		let Source::Env { reference } = source else {
			return Err(Error::Invalid(
				"Provider Credential Store is not configured".into(),
			));
		};
		aidash_domain::configuration::validate_endpoint(endpoint)?;
		let key = match reference {
			Some(name) => {
				aidash_domain::configuration::validate_secret_reference(name)?;
				self.credentials.resolve(name)?
			}
			None => String::new(),
		};
		Ok(Access {
			endpoint: endpoint.into(),
			bearer: key.into(),
		})
	}
}

/// Cloud calls inspect current metadata for the admitted ID. Plaintext reads and
/// broker routing belong exclusively to Issue #137; this adapter fails closed.
pub struct TenantAccess {
	pub environment: EnvironmentAccess,
	pub repository: Arc<dyn crate::provider_credentials::Repository>,
}
#[async_trait]
impl ProviderAccess for TenantAccess {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		let Source::Tenant { provider } = source else {
			return self.environment.resolve(context, endpoint, source).await;
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
		let id = context.provider_credential_id.ok_or_else(|| {
			Error::Invalid("Provider Credential access has no resolved ID".into())
		})?;
		let mut scope = self.repository.begin(&context.tenant).await?;
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
		row.require_active()?;
		scope.commit().await?;
		Err(Error::Invalid("credential broker not configured".into()))
	}
}
