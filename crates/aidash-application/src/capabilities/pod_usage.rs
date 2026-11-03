//! Deliver complete provider allocations independently of execution authority.
use crate::{Error, Result, ports::capabilities::pod_usage::*, usage::PodTime};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Default)]
pub struct Cursor {
	after: Option<Uuid>,
	instance: Option<Uuid>,
}
fn unavailable() -> Error {
	Error::External("allocation provider evidence or journal identity unavailable".into())
}
fn canonical(value: Value) -> Value {
	match value {
		Value::Object(values) => Value::Object(
			values
				.into_iter()
				.collect::<BTreeMap<_, _>>()
				.into_iter()
				.map(|(key, value)| (key, canonical(value)))
				.collect(),
		),
		Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
		value => value,
	}
}
fn timestamp(value: &str) -> Result<DateTime<Utc>> {
	DateTime::parse_from_rfc3339(value)
		.map(|at| at.with_timezone(&Utc))
		.map_err(|_| unavailable())
}
fn event(receipt: &AllocationReceipt, instance: Uuid) -> Result<PodTime> {
	if receipt.protocol != "aidash-allocation/1"
		|| receipt.runner_instance != instance
		|| receipt.allocation_id != receipt.evidence.pod_uid
		|| receipt.epoch == 0
		|| receipt.tenant.is_empty()
		|| receipt.tenant.len() > 255
		|| receipt.evidence.start_time != receipt.started_at
		|| receipt.evidence.containers.is_empty()
		|| receipt.evidence.containers.len() > 64
		|| receipt.digest.len() != 64
		|| !receipt
			.digest
			.bytes()
			.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
	{
		return Err(unavailable());
	}
	let started_at = timestamp(&receipt.started_at)?;
	let ended_at = timestamp(&receipt.ended_at)?;
	if ended_at < started_at {
		return Err(unavailable());
	}
	let mut names = BTreeSet::new();
	let mut latest = started_at;
	for container in &receipt.evidence.containers {
		if container.name.is_empty() || !names.insert(&container.name) {
			return Err(unavailable());
		}
		let finished = timestamp(&container.finished_at)?;
		if finished < started_at {
			return Err(unavailable());
		}
		latest = latest.max(finished);
	}
	if latest != ended_at {
		return Err(unavailable());
	}
	let mut value = serde_json::to_value(receipt)?;
	value
		.as_object_mut()
		.ok_or_else(unavailable)?
		.remove("digest");
	let digest = format!(
		"{:x}",
		Sha256::digest(serde_json::to_vec(&canonical(value))?)
	);
	if digest != receipt.digest {
		return Err(unavailable());
	}
	Ok(PodTime {
		allocation: receipt.allocation_id,
		receipt_digest: receipt.digest.clone(),
		tenant: receipt.tenant.clone(),
		started_at,
		ended_at,
	})
}
pub async fn sweep(repository: &dyn PodUsageRepository, cursor: &mut Cursor) -> Result<()> {
	let page = repository.pending_allocations(cursor.after).await?;
	if cursor
		.instance
		.is_some_and(|instance| instance != page.instance)
		|| page.receipts.len() > 16
		|| page
			.next_after
			.is_some_and(|after| Some(after) <= cursor.after)
	{
		return Err(unavailable());
	}
	cursor.instance = Some(page.instance);
	let mut previous = cursor.after;
	// Validate the entire bounded page before emitting any of its receipts.
	let mut events = Vec::with_capacity(page.receipts.len());
	for receipt in &page.receipts {
		if previous.is_some_and(|previous| receipt.allocation_id <= previous)
			|| page
				.next_after
				.is_some_and(|after| receipt.allocation_id > after)
		{
			return Err(unavailable());
		}
		events.push(event(receipt, page.instance)?);
		previous = Some(receipt.allocation_id);
	}
	let mut failure = None;
	for (receipt, event) in page.receipts.iter().zip(events) {
		if let Err(error) = repository.deliver_allocation(event).await {
			tracing::warn!(allocation = %receipt.allocation_id, %error, "allocation usage delivery failed");
			failure.get_or_insert(error);
			continue;
		}
		if let Err(error) = repository.acknowledge_allocation(receipt).await {
			tracing::warn!(allocation = %receipt.allocation_id, %error, "allocation usage acknowledgement failed");
			failure.get_or_insert(error);
		}
	}
	// Advance despite individual failures; wrapping retries retained receipts and new lower UUIDs.
	cursor.after = page.next_after;
	if let Some(error) = failure {
		return Err(error);
	}
	if page.pending_evidence > 0 {
		return Err(unavailable());
	}
	Ok(())
}
#[cfg(test)]
mod tests;
