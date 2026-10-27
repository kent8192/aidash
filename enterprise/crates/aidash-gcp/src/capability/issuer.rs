//! Worker-local Capability Token minting; metadata authority belongs to TenantAccess.
use aidash_application::{
	Error, Result,
	provider_access::{Access, Context, MaintenancePurpose, Operation, TokenIssuer},
};
use aidash_capability::{Claims, Maintenance, TOKEN_TTL_SECS, TokenSigner, TokenSubject};
use aidash_domain::provider_credentials::ProviderCredential;
use std::{
	sync::Arc,
	time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfiguration {
	pub endpoint: String,
	pub issuer: String,
	pub audience: String,
	pub kid: String,
}
impl WorkerConfiguration {
	pub fn validate(&self) -> Result<()> {
		aidash_domain::configuration::validate_endpoint(&self.endpoint)?;
		// Minted Capability Tokens are replayable bearer credentials until expiry.
		let url = reqwest::Url::parse(&self.endpoint)
			.map_err(|_| Error::Invalid("invalid Credential Broker endpoint URL".into()))?;
		let loopback = url
			.host_str()
			.map(|host| host.trim_start_matches('[').trim_end_matches(']'))
			.and_then(|host| host.parse::<std::net::IpAddr>().ok())
			.is_some_and(|ip| ip.is_loopback());
		if url.scheme() != "https" && !loopback {
			return Err(Error::Invalid(
				"Credential Broker endpoints require HTTPS except on literal loopback addresses"
					.into(),
			));
		}
		if !self.endpoint.ends_with("/api/v1")
			|| self.issuer.is_empty()
			|| self.issuer.len() > 256
			|| self.audience.is_empty()
			|| self.audience.len() > 16
			|| !self
				.audience
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		{
			return Err(Error::Invalid(
				"invalid Credential Broker worker configuration".into(),
			));
		}
		Ok(())
	}
}
pub struct CapabilityIssuer {
	configuration: WorkerConfiguration,
	project: String,
	signer: Arc<dyn TokenSigner>,
}
impl CapabilityIssuer {
	pub fn new(
		configuration: WorkerConfiguration,
		project: String,
		signer: Arc<dyn TokenSigner>,
	) -> Result<Self> {
		configuration.validate()?;
		if configuration.kid != signer.kid()
			|| project.is_empty()
			|| !project
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
		{
			return Err(Error::Invalid(
				"invalid Capability Token signer configuration".into(),
			));
		}
		Ok(Self {
			configuration,
			project,
			signer,
		})
	}
}
#[async_trait::async_trait]
impl TokenIssuer for CapabilityIssuer {
	async fn mint(&self, context: &Context, credential: &ProviderCredential) -> Result<Access> {
		let request = context.inference.as_ref().ok_or_else(|| {
			Error::Invalid("Capability Token has no approved inference scope".into())
		})?;
		aidash_domain::provider_credentials::validate_model_id(&request.model)?;
		if request.operations.is_empty()
			|| request.operations.len() > 3
			|| request
				.operations
				.iter()
				.enumerate()
				.any(|(i, op)| request.operations[..i].contains(op))
			// A Decision Attempt is alone in its token and has no completion bound.
			|| (request.operations.contains(&Operation::Decision)
				&& (request.operations.len() != 1
					|| request.max_output_tokens != 0
					|| request.cache_breakpoints))
			|| (!context.is_decision()
				&& (request.max_output_tokens == 0 || !request.hosts.is_empty()))
			|| context.tenant != credential.tenant
			|| credential.id.is_nil()
		{
			return Err(Error::Invalid(
				"invalid Capability Token inference scope".into(),
			));
		}
		let resource = format!(
			"projects/{}/secrets/aidash-{}-cred-{}",
			self.project, self.configuration.audience, credential.id
		);
		let version = credential
			.require_active()?
			.strip_prefix(&format!("{resource}/versions/"))
			.filter(|v| {
				!v.is_empty() && !v.starts_with('0') && v.bytes().all(|b| b.is_ascii_digit())
			})
			.ok_or_else(|| {
				Error::Invalid("Provider Credential has an invalid pinned version".into())
			})?;
		if credential.secret_resource != resource {
			return Err(Error::Invalid(
				"Secret Store resource does not match the environment".into(),
			));
		}
		let sub = match (context.run, context.maintenance, context.observation) {
			(Some(run), None, None) if !run.is_nil() => TokenSubject::Run {
				run: run.to_string(),
				call: request
					.call
					.filter(|call| !call.is_nil())
					.unwrap_or_else(uuid::Uuid::new_v4),
			},
			(None, Some(purpose), None) => TokenSubject::Maintenance {
				maintenance: match purpose {
					MaintenancePurpose::ProviderDiscovery => Maintenance::ProviderDiscovery,
					MaintenancePurpose::MemoryIndexing => Maintenance::MemoryIndexing,
					MaintenancePurpose::MemoryRetention => Maintenance::MemoryRetention,
					MaintenancePurpose::MemoryReflection => Maintenance::MemoryReflection,
					MaintenancePurpose::MemoryRetrieval => Maintenance::MemoryRetrieval,
				},
				tenant: context.tenant.clone(),
			},
			(None, None, Some(observation)) if !observation.is_nil() && context.is_decision() => {
				TokenSubject::ApprovalObservation {
					observation,
					tenant: context.tenant.clone(),
					call: request.call.filter(|call| !call.is_nil()).ok_or_else(|| {
						Error::Invalid("observation token requires its charged attempt".into())
					})?,
				}
			}
			_ => {
				return Err(Error::Invalid(
					"Capability Token requires one Run, Maintenance or approval observation scope"
						.into(),
				));
			}
		};
		let now = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_err(|_| Error::Invalid("Capability Token clock unavailable".into()))?
			.as_secs();
		let claims = Claims {
			data_region: context.data_protection.region()?,
			data_protection: context.data_protection.requirement(),
			iss: self.configuration.issuer.clone(),
			aud: self.configuration.audience.clone(),
			iat: now,
			exp: now + TOKEN_TTL_SECS,
			jti: uuid::Uuid::new_v4(),
			kid: self.signer.kid().into(),
			tenant: context.tenant.clone(),
			provider: credential.provider.id().into(),
			credential: credential.id,
			version: version.into(),
			sub,
			ops: request
				.operations
				.iter()
				.map(|op| match op {
					Operation::Chat => aidash_capability::Operation::Chat,
					Operation::Discovery => aidash_capability::Operation::Discovery,
					Operation::Embeddings => aidash_capability::Operation::Embeddings,
					Operation::Decision => aidash_capability::Operation::Decision,
				})
				.collect(),
			model: request.model.clone(),
			max_output_tokens: u64::from(request.max_output_tokens),
			cache_breakpoints: request.cache_breakpoints,
			hosts: request.hosts.clone(),
		};
		let bearer = aidash_capability::mint(&claims, self.signer.as_ref())
			.await
			.map_err(|_| Error::External("Capability Token signing unavailable".into()))?;
		tracing::info!(jti=%claims.jti, subject=?claims.sub, "Capability Token minted");
		Ok(Access {
			endpoint: self.configuration.endpoint.clone(),
			bearer,
			keyless: None,
		})
	}
}
