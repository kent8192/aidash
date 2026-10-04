//! Serialized attempt decisions, bounded refunds and idempotent settlement.
use crate::{
	Error, Result,
	ports::generation::settlement::{GenerationSettlementRepository, GenerationSettlementScope},
};
use aidash_domain::{
	generation::remote::{Attempt, Finalization, Settlement, Usage},
	semantic::Failure,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// Reservation and settlement use the same attempt lock and digest binding.
pub async fn lock_attempt(
	scope: &mut dyn GenerationSettlementScope,
	attempt: Uuid,
	digest: &str,
) -> Result<Option<Value>> {
	attempt_result(scope.lock_attempt(attempt, digest).await?, digest)
}

pub(super) fn attempt_result(saved: Attempt, digest: &str) -> Result<Option<Value>> {
	if saved.digest != digest {
		return Err(Error::Conflict(
			"provider attempt has a different reservation".into(),
		));
	}
	Ok(saved.result)
}

/// Unknown or excessive usage keeps its reservation. Report contract failure
/// only after the exact terminal decision and every local charge are durable.
pub async fn finalize(
	repository: &dyn GenerationSettlementRepository,
	usage: &Usage,
	result: &Finalization,
) -> Result<()> {
	usage.validate()?;
	let digest = usage.digest()?;
	let decision = result.accounting(usage.reserved_tokens);
	let mut scope = repository.begin().await?;
	finalize_in(scope.as_mut(), usage, result, &digest, &decision).await?;
	scope.commit().await?;
	if decision.provider_contract_violated {
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	}
	Ok(())
}

async fn finalize_in(
	scope: &mut dyn GenerationSettlementScope,
	usage: &Usage,
	result: &Finalization,
	digest: &str,
	decision: &Settlement,
) -> Result<()> {
	if let Some(previous) = lock_attempt(scope, usage.attempt_id, digest).await?
		&& previous != json!(result)
	{
		return Err(Error::Conflict(
			"provider attempt already finalized differently".into(),
		));
	}
	for row in scope.reservations(usage.attempt_id, digest).await? {
		if row.state != "RESERVED" {
			if row.state != decision.state || row.reported_tokens != decision.reported {
				return Err(Error::Conflict(
					"provider attempt already finalized differently".into(),
				));
			}
			continue;
		}
		if scope
			.refund_budget(
				row.request_id,
				decision.refund,
				usage.purpose,
				decision.release_call,
			)
			.await? != 1
		{
			return Err(Error::Conflict(
				"provider refund exceeds committed reservation".into(),
			));
		}
		let job = scope.request(row.request_id).await?;
		if job.quota_released
			&& scope
				.refund_policy(&job, decision.refund, usage.purpose, decision.release_call)
				.await? != 1
		{
			return Err(Error::Conflict(
				"provider refund exceeds policy allocation".into(),
			));
		}
		scope
			.settle_reservation(
				row.request_id,
				usage.attempt_id,
				decision.state,
				decision.reported,
			)
			.await?;
	}
	scope
		.save_finalization(usage.attempt_id, &json!(result))
		.await
}

#[cfg(test)]
mod tests;
