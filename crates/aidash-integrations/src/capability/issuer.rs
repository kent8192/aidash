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
		if request.model.is_empty()
			|| request.model.len() > 256
			|| request.model.split('/').any(|s| {
				s.is_empty()
					|| s == "." || s == ".."
					|| !s
						.bytes()
						.all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
			}) || request.operations.is_empty()
			|| request.operations.len() > 3
			|| request
				.operations
				.iter()
				.enumerate()
				.any(|(i, op)| request.operations[..i].contains(op))
			|| request.max_output_tokens == 0
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
				"Provider Credential Store resource does not match the environment".into(),
			));
		}
		let sub = match (context.run, context.maintenance) {
			(Some(run), None) if !run.is_nil() => TokenSubject::Run {
				run: run.to_string(),
				call: uuid::Uuid::new_v4(),
			},
			(None, Some(purpose)) => TokenSubject::Maintenance {
				maintenance: match purpose {
					MaintenancePurpose::MemoryIndexing => Maintenance::MemoryIndexing,
					MaintenancePurpose::MemoryRetention => Maintenance::MemoryRetention,
					MaintenancePurpose::MemoryReflection => Maintenance::MemoryReflection,
					MaintenancePurpose::MemoryRetrieval => Maintenance::MemoryRetrieval,
				},
				tenant: context.tenant.clone(),
			},
			_ => {
				return Err(Error::Invalid(
					"Capability Token requires a Run or Maintenance purpose".into(),
				));
			}
		};
		let now = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map_err(|_| Error::Invalid("Capability Token clock unavailable".into()))?
			.as_secs();
		let claims = Claims {
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
				})
				.collect(),
			model: request.model.clone(),
			max_output_tokens: u64::from(request.max_output_tokens),
		};
		let bearer = aidash_capability::mint(&claims, self.signer.as_ref())
			.await
			.map_err(|_| Error::External("Capability Token signing unavailable".into()))?;
		tracing::info!(jti=%claims.jti, subject=?claims.sub, "Capability Token minted");
		Ok(Access {
			endpoint: self.configuration.endpoint.clone(),
			bearer,
		})
	}
}
