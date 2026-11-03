//! Reserve, invoke, and settle embedding usage under one current authority scope.
use crate::{
	Error, Result,
	ports::{
		EmbeddingProvider,
		semantic::embedding::{EmbeddingAllowance, SemanticEmbeddingScope},
	},
};
use aidash_domain::{
	generation::embedding::Origin,
	semantic::{EmbeddingConfig, Failure},
};
use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
impl EmbeddingAllowance for crate::generation::embedding::Reservation {
	async fn settle(self: Box<Self>, reported: Option<u64>) -> Result<()> {
		Self::settle(*self, reported).await
	}
}
/// Provider failure keeps the committed reservation conservatively charged.
pub async fn invoke(
	scope: &mut dyn SemanticEmbeddingScope,
	provider: &dyn EmbeddingProvider,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: Origin,
) -> Result<Vec<f32>> {
	invoke_input(
		scope,
		provider,
		workspace,
		config,
		aidash_domain::semantic::EmbeddingInput::Text(text),
		origin,
	)
	.await
}

pub async fn invoke_input(
	scope: &mut dyn SemanticEmbeddingScope,
	provider: &dyn EmbeddingProvider,
	workspace: Uuid,
	config: &EmbeddingConfig,
	input: aidash_domain::semantic::EmbeddingInput<'_>,
	origin: Origin,
) -> Result<Vec<f32>> {
	input.validate(config)?;
	let reservation = scope.reserve(workspace, config, input, origin).await?;
	let output = provider
		.embed(config, input)
		.await
		.map_err(|error| match error {
			// A rejected key (401–403) or a broken response contract is not an
			// outage: keep its typed reason so the Run pauses instead of retrying.
			Error::RemoteSemantic(reason) if !reason.transient() => error,
			// A Keyless endpoint the Deployment does not permit (ADR 0036) is
			// refused, not unavailable.
			Error::Forbidden => error,
			// Tenant access failures are admission/state failures, never a retryable
			// provider outage. Preserve them without changing self-hosted diagnostics.
			Error::Invalid(_) | Error::NotFound(_) | Error::Conflict(_)
				if config.provider_credential.is_some() =>
			{
				error
			}
			// Transport failures, timeouts and 408/429/5xx are a transient outage,
			// typed so a Run retries them; nothing else is (#233).
			Error::RemoteSemantic(_) | Error::External(_) => {
				Error::RemoteSemantic(Failure::Unavailable)
			}
			_ => Error::SemanticUnavailable,
		})?;
	if let Some(reservation) = reservation {
		reservation.settle(output.tokens).await?;
	}
	Ok(output.vector)
}
#[cfg(test)]
mod tests;
