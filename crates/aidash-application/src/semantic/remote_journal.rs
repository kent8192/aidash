//! Attempt admission, retries, durable dispatch and manual recovery share portable journal rules.
use crate::{
	Error, Result,
	ports::semantic::remote_journal::{JournalRepository, JournalScope},
};
use aidash_domain::semantic::{
	Failure,
	remote::{
		Binding, Operation, Receipt,
		journal::{Attempt, Claim, ClaimPlan, Record, claim_plan, retry},
	},
};
use serde_json::{Value, json};
use uuid::Uuid;
fn code(reason: Failure) -> Result<String> {
	serde_json::to_value(reason)?
		.as_str()
		.map(str::to_owned)
		.ok_or(Error::RemoteSemantic(Failure::ProviderContract))
}
pub async fn prepare(
	repository: &dyn JournalRepository,
	operation: &Operation,
	binding: &Binding,
) -> Result<Record> {
	operation.validate()?;
	let digest = operation.digest()?;
	let value = json!({"operation":operation,"semantic":binding});
	let mut scope = repository.begin().await?;
	scope.insert_binding(operation, &digest, &value).await?;
	let record = scope.shared(operation.id).await?;
	if record.home_node != operation.home_node
		|| record.grant_id != operation.grant_id
		|| record.admission_id != operation.admission_id
		|| record.digest != digest
		|| record.binding != value
	{
		return Err(Error::Conflict(
			"semantic operation id already binds different inputs".into(),
		));
	};
	scope.commit().await?;
	Ok(record)
}
pub async fn bound(
	repository: &dyn JournalRepository,
	operation: &Operation,
	binding: &Binding,
) -> Result<Record> {
	let record = repository.bound(operation.id).await?;
	if record.digest != operation.digest()?
		|| record.binding != json!({"operation":operation,"semantic":binding})
	{
		return Err(Error::Forbidden);
	};
	Ok(record)
}
pub async fn claim(repository: &dyn JournalRepository, id: Uuid) -> Result<Claim> {
	let mut scope = repository.begin().await?;
	let record = scope.locked(id).await?;
	let now = scope.clock().await?;
	match claim_plan(&record, now) {
		ClaimPlan::Ready => {
			let receipt = serde_json::from_value(
				record
					.receipt
					.ok_or(Error::RemoteSemantic(Failure::ProviderContract))?,
			)?;
			scope.commit().await?;
			return Ok(Claim::Ready(Box::new(receipt)));
		}
		ClaimPlan::Rejected(failure) => return Err(Error::RemoteSemantic(failure)),
		ClaimPlan::Expired { failures, delay } => {
			if let Some(previous) = record.attempt_id {
				scope.expire_attempt(previous).await?
			};
			let failure = if delay.is_some() {
				Failure::Unavailable
			} else {
				Failure::RetriesExhausted
			};
			let state = if delay.is_some() { "WAITING" } else { "PAUSED" };
			scope
				.wait_expired(id, failures, delay, state, &code(failure)?)
				.await?;
			scope.commit().await?;
			return Err(Error::RemoteSemantic(if delay.is_some() {
				Failure::Pending
			} else {
				Failure::RetriesExhausted
			}));
		}
		ClaimPlan::Fresh => {}
	}
	let attempt = Attempt {
		operation_id: id,
		id: Uuid::new_v4(),
		fence: record.fence + 1,
	};
	scope.activate(id, &attempt, &record).await?;
	scope.commit().await?;
	Ok(Claim::Attempt(attempt))
}
pub async fn dispatched(
	repository: &dyn JournalRepository,
	attempt: &Attempt,
	reservations: &Value,
) -> Result<()> {
	let mut scope = repository.begin().await?;
	scope.current(attempt).await?;
	if scope.dispatched(attempt, reservations).await? != 1 {
		return Err(Error::RemoteSemantic(Failure::Pending));
	};
	scope.commit().await
}
pub async fn complete(
	repository: &dyn JournalRepository,
	attempt: &Attempt,
	receipt: &Receipt,
) -> Result<()> {
	let mut scope = repository.begin().await?;
	let record = scope.current(attempt).await?;
	if record.id != receipt.operation_id
		|| record.digest != receipt.operation_digest
		|| record.home_node != receipt.home_node
		|| record.grant_id != receipt.grant_id
		|| record.admission_id != receipt.admission_id
	{
		return Err(Error::RemoteSemantic(Failure::ProviderContract));
	};
	for source in &receipt.sources {
		scope.record_source(receipt, source).await?
	}
	scope.complete_operation(receipt).await?;
	scope.complete_attempt(attempt).await?;
	scope.commit().await
}
pub async fn failed(
	repository: &dyn JournalRepository,
	attempt: &Attempt,
	failure: Failure,
) -> Result<Failure> {
	let mut scope = repository.begin().await?;
	let record = scope.current(attempt).await?;
	let retry = retry(record.failures, failure);
	let error = code(retry.failure)?;
	scope
		.fail_operation(&record, retry.failures, retry.delay, retry.state, &error)
		.await?;
	scope.fail_attempt(attempt, &error).await?;
	scope.commit().await?;
	Ok(retry.failure)
}
pub async fn resume(
	scope: &mut dyn JournalScope,
	grant: Uuid,
	admission: Uuid,
	workspace: Uuid,
	actor: &str,
) -> Result<()> {
	for record in scope.resume_records(grant, admission).await? {
		if record.state == "INVALIDATED" {
			return Err(Error::RemoteSemantic(Failure::Invalidated));
		};
		if record.state == "CANCELLED" {
			return Err(Error::RemoteSemantic(Failure::Authority));
		};
		if !matches!(record.state.as_str(), "PAUSED" | "WAITING") {
			continue;
		};
		scope.resume_record(&record).await?;
		scope.event(workspace,"semantic.manual_retry",json!({"operation_id":record.id,"grant_id":grant,"admission_id":admission,"cycle":record.cycle+1,"actor":actor,"reason":"authorized_resume"})).await?;
	}
	Ok(())
}
