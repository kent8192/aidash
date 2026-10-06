//! Durable local inference charges and settlement precede returned provider results.
use crate::{
	Error, Result,
	ports::generation::inference::{GenerationInferenceAuthority, GenerationInferenceRepository},
};
use aidash_domain::provider::{ContentPart, ModelRequest, ModelResponse};
use std::sync::Arc;
use uuid::Uuid;

pub struct Reservation {
	repository: Arc<dyn GenerationInferenceRepository>,
	attempt: Uuid,
	requests: Vec<Uuid>,
	amount: i64,
}
impl Reservation {
	pub fn check_request(window: usize, request: &ModelRequest) -> Result<()> {
		request.ensure_fits(window).map_err(Into::into)
	}
	pub fn check_request_with_parts(
		window: usize,
		request: &ModelRequest,
		parts: &[ContentPart],
	) -> Result<()> {
		request
			.ensure_fits_with_parts(window, parts)
			.map_err(Into::into)
	}
	/// Consuming the receipt keeps a provider response from refunding twice.
	pub async fn settle(self, response: &ModelResponse) -> Result<()> {
		let accounting = aidash_domain::generation::inference::accounting(self.amount, response);
		let mut transaction = self.repository.begin().await?;
		for request in self.requests {
			transaction.refund(request, accounting.refund).await?;
			if accounting.refund > 0
				&& let Some((tenant, policy)) = transaction.released_policy(request).await?
			{
				transaction
					.refund_allocated(&tenant, &policy, accounting.refund)
					.await?;
			}
			transaction
				.report(request, self.attempt, accounting.reported)
				.await?;
		}
		transaction.commit().await?;
		if accounting.exceeded {
			return Err(Error::External(
				"provider usage exceeded reserved model limits".into(),
			));
		}
		Ok(())
	}
}
/// Charges all local ancestors before returning a durable reservation.
pub async fn reserve(
	scope: &mut dyn GenerationInferenceAuthority,
	repository: Arc<dyn GenerationInferenceRepository>,
	run: Uuid,
	attempt: Uuid,
	window: usize,
	output: u32,
) -> Result<Option<Reservation>> {
	reserve_many(&mut [scope], repository, run, attempt, window, output).await
}

/// One provider call charges the union of every contributing origin's ancestors.
/// Shared ancestors occur once and every charge commits or rolls back together.
pub async fn reserve_many(
	scopes: &mut [&mut dyn GenerationInferenceAuthority],
	repository: Arc<dyn GenerationInferenceRepository>,
	run: Uuid,
	attempt: Uuid,
	window: usize,
	output: u32,
) -> Result<Option<Reservation>> {
	let mut requests = std::collections::BTreeSet::new();
	for scope in scopes {
		requests.extend(scope.requests(repository.node_id()).await?);
	}
	let requests: Vec<_> = requests.into_iter().collect();
	if requests.is_empty() {
		return Ok(None);
	}
	let amount = aidash_domain::generation::inference::reservation_amount(window, output)?;
	let mut transaction = repository.begin().await?;
	for request in &requests {
		transaction.charge(*request, amount).await?;
		transaction.reserve(*request, attempt, run, amount).await?;
	}
	transaction.commit().await?;
	Ok(Some(Reservation {
		repository,
		attempt,
		requests,
		amount,
	}))
}
#[cfg(test)]
mod tests;
