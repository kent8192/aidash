//! Durable admission and settlement prevent duplicate provider dispatches.
use crate::{
	Error, Result,
	ports::generation::dispatch::{GenerationDispatchRepository, GenerationDispatchSettlement},
};
use aidash_domain::{
	generation::{
		dispatch::{FinalizeInput, Input, Record},
		remote::{Finalization, Reserved},
	},
	semantic::Failure,
};
use serde_json::json;
use uuid::Uuid;

pub async fn prepare(
	repository: &dyn GenerationDispatchRepository,
	input: &Input,
	peer: &str,
) -> Result<()> {
	input.usage.validate()?;
	if input.usage.dispatcher_node != repository.node_id()
		|| serde_json::to_vec(&input.boundary)?.len() > 65536
	{
		return Err(Error::Forbidden);
	}
	let digest = input.usage.digest()?;
	let mut preparation = repository.begin_preparation().await?;
	preparation.insert(input, peer, &digest).await?;
	let record = preparation.record(input.usage.attempt_id).await?;
	if record.digest != digest
		|| record.boundary != input.boundary
		|| record.peer_node != peer
		|| record.usage != json!(input.usage)
	{
		return Err(Error::Conflict("provider admission binding changed".into()));
	}
	if record.state != "PREPARING" {
		return Err(Error::RemoteSemantic(Failure::Pending));
	}
	preparation.commit().await
}

pub async fn bound(repository: &dyn GenerationDispatchRepository, input: &Input) -> Result<Record> {
	let record = repository
		.record(input.usage.attempt_id)
		.await?
		.ok_or(Error::Forbidden)?;
	if record.digest != input.usage.digest()?
		|| record.boundary != input.boundary
		|| record.usage != json!(input.usage)
	{
		return Err(Error::Forbidden);
	}
	Ok(record)
}

pub async fn admitted(
	repository: &dyn GenerationDispatchRepository,
	input: &Input,
	receipts: &[Reserved],
) -> Result<()> {
	if repository.admit(input, receipts).await? != 1 {
		return Err(Error::RemoteSemantic(Failure::Pending));
	}
	Ok(())
}

/// Persist the terminal decision before issuing any refund to either owner.
pub async fn finish(
	repository: &dyn GenerationDispatchRepository,
	settlement: &dyn GenerationDispatchSettlement,
	input: &Input,
	result: Finalization,
) -> Result<()> {
	let record = bound(repository, input).await?;
	let (from, state) = match result {
		Finalization::Aborted {} => ("PREPARING", "ABORTED"),
		Finalization::Settled { .. } => ("DISPATCHED", "SETTLED"),
	};
	let value = json!(result);
	if repository
		.finalize(input.usage.attempt_id, from, state, &value)
		.await?
		!= 1 && (record.state != state || record.finalization.as_ref() != Some(&value))
	{
		return Err(Error::Conflict(
			"provider dispatch cannot be finalized in this state".into(),
		));
	}
	deliver(repository, settlement, input.usage.attempt_id).await
}

pub async fn deliver(
	repository: &dyn GenerationDispatchRepository,
	settlement: &dyn GenerationDispatchSettlement,
	attempt: Uuid,
) -> Result<()> {
	let mut visibility = repository.begin_visibility().await?;
	let record = visibility
		.terminal_record(attempt)
		.await?
		.ok_or(Error::Forbidden)?;
	let input = FinalizeInput {
		usage: serde_json::from_value(record.usage)?,
		result: serde_json::from_value(record.finalization.ok_or(Error::Forbidden)?)?,
	};
	let local = settlement.local(&input.usage, &input.result).await;
	visibility.suspend().await?;
	if !record.peer_finalized {
		let _ = settlement.peer(&record.peer_node, &input).await?;
		if !matches!(
			&local,
			Ok(()) | Err(Error::RemoteSemantic(Failure::ProviderContract))
		) {
			return local;
		}
		repository.mark_peer_finalized(attempt).await?;
	}
	local
}

/// Replay durable decisions after restart without contacting any provider.
pub async fn reconcile(
	repository: &dyn GenerationDispatchRepository,
	settlement: &dyn GenerationDispatchSettlement,
) -> Result<()> {
	let mut visibility = repository.begin_visibility().await?;
	visibility.abort_stale_preparations().await?;
	let ids = visibility.pending().await?;
	visibility.suspend().await?;
	for id in ids {
		// Each failed delivery remains in the bounded, durable retry outbox.
		let _ = deliver(repository, settlement, id).await;
	}
	Ok(())
}

#[cfg(test)]
mod tests;
