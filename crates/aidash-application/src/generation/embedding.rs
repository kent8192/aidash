//! Pinned embedding authority and durable ancestor accounting apply to every caller.
use crate::{
	Error, Result,
	authorization::catalog,
	ports::generation::embedding::{GenerationEmbeddingAuthority, GenerationEmbeddingRepository},
};
use aidash_domain::{
	generation::embedding::{self, Attempt},
	registry::EntityRef,
	semantic::EmbeddingConfig,
};
use std::sync::Arc;
use uuid::Uuid;

pub use aidash_domain::generation::embedding::Origin;

pub struct Reservation {
	repository: Arc<dyn GenerationEmbeddingRepository>,
	requests: Vec<Uuid>,
	attempt: Uuid,
	amount: i64,
}

impl Reservation {
	/// Consuming a receipt prevents the same response from refunding twice.
	pub async fn settle(self, reported: Option<u64>) -> Result<()> {
		let accounting = embedding::accounting(self.amount, reported);
		let mut transaction = self.repository.begin().await?;
		for request in self.requests {
			transaction.refund(request, accounting.refund).await?;
			transaction
				.report(request, self.attempt, accounting.reported)
				.await?;
		}
		transaction.commit().await?;
		if accounting.exceeded {
			return Err(Error::Invalid(
				"embedding usage exceeded reserved input limits".into(),
			));
		}
		Ok(())
	}
}

/// Validate inherited authority and all provider pins before charging any ancestor.
pub async fn reserve(
	scope: &mut dyn GenerationEmbeddingAuthority,
	repository: Arc<dyn GenerationEmbeddingRepository>,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: Origin,
) -> Result<Option<Reservation>> {
	let jobs = scope.requests(repository.node_id()).await?;
	let Some(first) = jobs.first() else {
		return Ok(None);
	};
	super::publication::require_live(
		scope.live(),
		repository.node_id(),
		first.task_id,
		&EntityRef {
			id: first.agent_id.clone(),
			version: first.agent_version.clone(),
		},
	)
	.await?;
	let mut reference = None;
	for job in &jobs {
		let approved = scope.pinned_policy(job).await?.embedding.ok_or_else(|| {
			Error::Invalid(
				"generated semantic memory requires an approved embedding provider".into(),
			)
		})?;
		if reference
			.as_ref()
			.is_some_and(|reference| reference != &approved.provider)
		{
			return Err(Error::Forbidden);
		}
		reference = Some(approved.provider);
	}
	let reference = reference.ok_or(Error::Forbidden)?;
	catalog::entry(scope.catalog(), &reference, "registry.read").await?;
	let entry = catalog::entry(scope.catalog(), &reference, "embedding.invoke").await?;
	if entry.kind != "embedding"
		|| serde_json::from_value::<EmbeddingConfig>(entry.config)? != *config
	{
		return Err(Error::Forbidden);
	}
	let amount = embedding::reservation_amount(text.len())?;
	let attempt = Attempt {
		id: Uuid::new_v4(),
		workspace,
		origin,
		provider: reference,
		request_bytes: text.len() as i64,
		amount,
	};
	let mut transaction = repository.begin().await?;
	for job in &jobs {
		transaction.charge(job.id, amount).await?;
		transaction.reserve(job.id, &attempt).await?;
	}
	transaction.commit().await?;
	Ok(Some(Reservation {
		repository,
		requests: jobs.iter().map(|job| job.id).collect(),
		attempt: attempt.id,
		amount,
	}))
}

#[cfg(test)]
mod tests;
