//! Reserve, invoke, and settle embedding usage under one current authority scope.
use crate::{
	Error, Result,
	ports::{
		EmbeddingProvider,
		semantic::embedding::{EmbeddingAllowance, SemanticEmbeddingScope},
	},
};
use aidash_domain::{generation::embedding::Origin, semantic::EmbeddingConfig};
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
	let reservation = scope.reserve(workspace, config, text, origin).await?;
	let output = provider
		.embed(config, text)
		.await
		.map_err(|_| Error::SemanticUnavailable)?;
	if let Some(reservation) = reservation {
		reservation.settle(output.tokens).await?;
	}
	Ok(output.vector)
}
#[cfg(test)]
mod tests;
