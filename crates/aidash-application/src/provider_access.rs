//! Per-call provider access. Key Material never crosses Registry or federation metadata.
use crate::{Error, Result};
use aidash_domain::keyless::{Destination, PermittedOrigins};
use async_trait::async_trait;
use secrecy::SecretString;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct Context {
	/// Current policy evaluated immediately before this call; never an admitted snapshot.
	pub data_protection: aidash_domain::data_protection::DataProtection,
	pub tenant: String,
	pub run: Option<Uuid>,
	/// Independent approval observation; never an executing Run identity.
	pub observation: Option<Uuid>,
	/// Local maintenance authority when no Run exists; used by #137's Token Subject.
	pub maintenance: Option<MaintenancePurpose>,
	/// Admission pins an ID, not a Secret Manager version or a mutable binding.
	pub provider_credential_id: Option<Uuid>,
	/// Approved per-invocation model and bounds; populated by the inference adapter.
	pub inference: Option<Inference>,
}
#[derive(Debug, Clone)]
pub struct Inference {
	pub provider_tools: Vec<aidash_domain::tool::provider_tools::PinnedTool>,
	pub model: String,
	pub operations: Vec<Operation>,
	pub max_output_tokens: u32,
	/// Requests the signed authority to send explicit `cache_control` breakpoints.
	pub cache_breakpoints: bool,
	/// The caller's identifier for this provider call, such as a Decision
	/// Attempt's; the token issuer generates one when it is unset.
	pub call: Option<Uuid>,
	/// The Decider Route's OpenRouter hosts, which a Decisions API attempt's
	/// token signs; empty for every other call (ADR 0066).
	pub hosts: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
	Chat,
	Discovery,
	Embeddings,
	/// One System One request: a Decision Attempt of a Run (ADR 0041).
	Decision,
}
impl Context {
	/// A Decision Attempt uses the Tenant's current binding rather than the Run's
	/// admitted pin: Runs on a Plan Connection or a Keyless endpoint have no
	/// OpenRouter pin, and compaction is not inference (ADR 0041).
	pub fn is_decision(&self) -> bool {
		self.inference
			.as_ref()
			.is_some_and(|inference| inference.operations == [Operation::Decision])
	}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenancePurpose {
	/// Read-only provider catalogs for current sends and authorized policy previews.
	ProviderDiscovery,
	MemoryIndexing,
	MemoryRetention,
	MemoryReflection,
	MemoryRetrieval,
}
/// Where a model-provider call gets its bearer. There is no environment source:
/// model-provider keys come only from Tenant Provider Credentials (ADR 0006),
/// and plan usage only from the Run owner's Plan Connection (ADR 0007).
#[derive(Debug, Clone)]
pub enum Source {
	/// The configured endpoint is called without a key.
	Keyless,
	/// The Tenant's Provider Credential for this Provider Catalog entry.
	Tenant { provider: String },
	/// The Plan Connection admission pinned for the Run, owned by the subject
	/// at the root of its delegation chain. Never a Provider Credential.
	Plan { provider: String },
}
impl Source {
	/// A configuration names at most one grant; validation rejects both.
	pub fn configured(
		provider_credential: &Option<String>,
		plan_connection: &Option<String>,
	) -> Self {
		match (provider_credential, plan_connection) {
			(_, Some(provider)) => Self::Plan {
				provider: provider.clone(),
			},
			(Some(provider), None) => Self::Tenant {
				provider: provider.clone(),
			},
			(None, None) => Self::Keyless,
		}
	}
}
#[derive(Debug)]
pub struct Access {
	pub endpoint: String,
	pub bearer: SecretString,
	/// How a Keyless endpoint may connect (ADR 0036); `None` for a Provider
	/// Catalog endpoint.
	pub keyless: Option<Destination>,
}
#[async_trait]
pub trait ProviderAccess: Send + Sync {
	/// Reload current Tenant and delegation-chain policy before constructing a request.
	async fn data_protection(
		&self,
		context: &Context,
	) -> Result<aidash_domain::data_protection::DataProtection> {
		Ok(context.data_protection)
	}

	/// Resolve read-only catalog authority while preserving the caller's purpose.
	/// Native adapters authorize the original Decision/Run before narrowing it.
	async fn discovery(
		&self,
		context: &Context,
		endpoint: &str,
		source: &Source,
		model: &str,
	) -> Result<Access> {
		let mut discovery = context.clone();
		discovery.inference = Some(Inference {
			provider_tools: vec![],
			model: model.into(),
			operations: vec![Operation::Discovery],
			max_output_tokens: 1,
			cache_breakpoints: false,
			call: None,
			hosts: vec![],
		});
		self.resolve(&discovery, endpoint, source).await
	}
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access>;
	/// Whether `resolve` would find an active credential, without minting a
	/// Capability Token or reading Key Material. Callers use it to fail before
	/// they reserve allowances. There is no default: `resolve` does both.
	async fn check(&self, context: &Context, endpoint: &str, source: &Source) -> Result<()>;
	/// Reports a rejected plan call so the Run's Plan Connection can be
	/// refreshed or marked ineligible. Returns `true` when a forced refresh
	/// produced a new token that the caller may retry with once.
	async fn plan_rejected(
		&self,
		_context: &Context,
		_disposition: aidash_domain::plan_connections::Disposition,
	) -> Result<bool> {
		Ok(false)
	}
}
/// Access without a Secret Store: Keyless endpoints only, limited
/// to public addresses except the Deployment's permitted private origins.
#[derive(Debug, Clone, Default)]
pub struct KeylessAccess {
	/// Shared by every clone. Production code sets it once at startup; only
	/// test builds can add to it later (`permit_origin_of`).
	origins: Arc<std::sync::RwLock<PermittedOrigins>>,
	/// Explicit Deployment ownership declaration, separate from network access.
	hosted_origins: Arc<PermittedOrigins>,
	deployment_region: Option<aidash_domain::data_protection::Region>,
}
impl KeylessAccess {
	pub fn with_deployment_region(
		mut self,
		region: Option<aidash_domain::data_protection::Region>,
	) -> Self {
		self.deployment_region = region;
		self
	}
	pub fn deployment_region(&self) -> Option<aidash_domain::data_protection::Region> {
		self.deployment_region
	}
	pub fn permitting(origins: PermittedOrigins) -> Self {
		Self {
			origins: Arc::new(std::sync::RwLock::new(origins)),
			hosted_origins: Arc::default(),
			deployment_region: None,
		}
	}
	pub fn with_deployment_hosted(mut self, origins: PermittedOrigins) -> Self {
		self.hosted_origins = Arc::new(origins);
		self
	}
	pub fn data_protection_destination(
		&self,
		endpoint: &str,
	) -> Result<aidash_domain::data_protection::Destination<'static>> {
		let origin = url::Url::parse(endpoint)
			.map_err(|_| Error::Invalid("invalid model endpoint URL".into()))?
			.origin()
			.ascii_serialization();
		Ok(if self.hosted_origins.permits(&origin) {
			aidash_domain::data_protection::Destination::DeploymentHosted(self.deployment_region)
		} else {
			aidash_domain::data_protection::Destination::TenantKeyless
		})
	}
	/// Permit the origin of `url` for this access and every clone of it, as an
	/// Deployment Administrator would permit a private model server. Test fixtures start after
	/// the runtime that holds this access, so they cannot be listed up front.
	#[cfg(any(test, feature = "test-support"))]
	pub fn permit_origin_of(&self, url: &str) -> Result<()> {
		let origin = url::Url::parse(url)
			.map_err(|_| Error::Invalid("invalid fixture URL".into()))?
			.origin()
			.ascii_serialization();
		let added = PermittedOrigins::parse([origin])?;
		let mut origins = self.origins.write().expect("Keyless origins lock");
		*origins = std::mem::take(&mut *origins).with(added);
		Ok(())
	}
	/// How `endpoint` may connect under the Deployment's permitted origins.
	pub fn destination(&self, endpoint: &str) -> Result<Destination> {
		let origins = self.origins.read().expect("Keyless origins lock");
		Ok(aidash_domain::keyless::destination(endpoint, &origins)?)
	}

	/// The permitted origins, so that a test can configure a child process alike.
	#[cfg(any(test, feature = "test-support"))]
	pub fn permitted_origins(&self) -> Vec<String> {
		let origins = self.origins.read().expect("Keyless origins lock");
		origins.iter().map(str::to_owned).collect()
	}
}
#[async_trait]
impl ProviderAccess for KeylessAccess {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		match source {
			Source::Keyless => {}
			Source::Tenant { .. } => {
				return Err(Error::Invalid("Secret Store is not configured".into()));
			}
			Source::Plan { .. } => {
				return Err(Error::Plan(
					aidash_domain::plan_connections::Failure::NotConnected,
				));
			}
		}
		let destination = self.destination(endpoint)?;
		if destination == Destination::Forbidden {
			tracing::warn!(
				"Keyless endpoint refused: a non-public address outside the permitted private origins"
			);
			return Err(Error::Forbidden);
		}
		context
			.data_protection
			.permits(self.data_protection_destination(endpoint)?)?;
		Ok(Access {
			endpoint: endpoint.into(),
			bearer: String::new().into(),
			keyless: Some(destination),
		})
	}
	/// Keyless access holds no Key Material and mints no token, so checking it
	/// is resolving it.
	async fn check(&self, context: &Context, endpoint: &str, source: &Source) -> Result<()> {
		self.resolve(context, endpoint, source).await.map(|_| ())
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
impl TenantAccess {
	/// Current metadata and pinned version of the credential a call may use.
	async fn active(
		&self,
		context: &Context,
		endpoint: &str,
		provider: &str,
	) -> Result<(
		aidash_domain::provider_credentials::ProviderCredential,
		String,
	)> {
		let provider = aidash_domain::provider_credentials::Provider::credential(provider)?;
		if context.tenant.is_empty()
			|| (usize::from(context.run.is_some())
				+ usize::from(context.maintenance.is_some())
				+ usize::from(context.observation.is_some())
				!= 1) || context.observation.is_some_and(|id| id.is_nil())
			|| (context.observation.is_some() && !context.is_decision())
			|| (context.is_decision() && context.run.is_none() && context.observation.is_none())
			|| endpoint != provider.base_url()
		{
			return Err(Error::Invalid(
				"Provider Credential requires a local Tenant and Run or maintenance purpose".into(),
			));
		}
		let mut scope = self.repository.begin(&context.tenant).await?;
		let id = if (context.run.is_some() && !context.is_decision())
			|| (context.maintenance == Some(MaintenancePurpose::ProviderDiscovery)
				&& context.provider_credential_id.is_some())
		{
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
		Ok((row, version))
	}
}
#[async_trait]
impl ProviderAccess for TenantAccess {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		let Source::Tenant { provider } = source else {
			return self.keyless.resolve(context, endpoint, source).await;
		};
		let (row, version) = self.active(context, endpoint, provider).await?;
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
			endpoint: context
				.data_protection
				.region()?
				.map(|region| region.openrouter_endpoint())
				.transpose()?
				.unwrap_or(row.provider.base_url())
				.into(),
			bearer,
			keyless: None,
		})
	}
	async fn check(&self, context: &Context, endpoint: &str, source: &Source) -> Result<()> {
		let Source::Tenant { provider } = source else {
			return self.keyless.check(context, endpoint, source).await;
		};
		if self.issuer.is_none() && self.reader.is_none() {
			return Err(Error::Invalid("credential broker not configured".into()));
		}
		self.active(context, endpoint, provider).await.map(|_| ())
	}
}
#[cfg(test)]
mod tests;
