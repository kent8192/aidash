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

/// An authority-checked input to a single atomic reservation; callers cannot mint it.
pub struct Approved {
	node: String,
	provider: EntityRef,
	requests: Vec<Uuid>,
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
	reserve_many(&mut [scope], repository, workspace, config, text, origin).await
}

/// Validate each contributing lease before atomically charging their ancestor union.
pub async fn reserve_many(
	scopes: &mut [&mut dyn GenerationEmbeddingAuthority],
	repository: Arc<dyn GenerationEmbeddingRepository>,
	workspace: Uuid,
	config: &EmbeddingConfig,
	text: &str,
	origin: Origin,
) -> Result<Option<Reservation>> {
	let mut approved = vec![];
	for scope in scopes {
		if let Some(origin) = authorize(*scope, repository.node_id(), config).await? {
			approved.push(origin);
		}
	}
	reserve_approved(approved, repository, workspace, text, origin).await
}

/// All checks precede this transaction, including when native callers suspend leases.
pub async fn reserve_approved(
	approved: Vec<Approved>,
	repository: Arc<dyn GenerationEmbeddingRepository>,
	workspace: Uuid,
	text: &str,
	origin: Origin,
) -> Result<Option<Reservation>> {
	let mut requests = std::collections::BTreeSet::new();
	let mut reference = None;
	for Approved {
		node,
		provider,
		requests: jobs,
	} in approved
	{
		if node != repository.node_id() {
			return Err(Error::Forbidden);
		}
		if reference
			.as_ref()
			.is_some_and(|approved| approved != &provider)
		{
			return Err(Error::Forbidden);
		}
		reference = Some(provider);
		requests.extend(jobs);
	}
	let Some(reference) = reference else {
		return Ok(None);
	};
	let requests: Vec<_> = requests.into_iter().collect();
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
	for request in &requests {
		transaction.charge(*request, amount).await?;
		transaction.reserve(*request, &attempt).await?;
	}
	transaction.commit().await?;
	Ok(Some(Reservation {
		repository,
		requests,
		attempt: attempt.id,
		amount,
	}))
}

pub async fn authorize(
	scope: &mut dyn GenerationEmbeddingAuthority,
	node: &str,
	config: &EmbeddingConfig,
) -> Result<Option<Approved>> {
	let jobs = scope.requests(node).await?;
	let Some(first) = jobs.first() else {
		return Ok(None);
	};
	super::publication::require_live(
		scope.live(),
		node,
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
	Ok(Some(Approved {
		node: node.to_owned(),
		provider: reference,
		requests: jobs.iter().map(|job| job.id).collect(),
	}))
}

#[cfg(test)]
mod tests;
