//! Immutable durable obligations, not a route prefix or caller-selected recovery flag.
use crate::{
	Result,
	apps::federation::transactions::models::{AtomicCoordinator, AtomicParticipant},
};
use aidash_application::federation::inbound::RequestDigest;
use aidash_domain::{nodes::NodeAddress, transactions::Manifest};
use reinhardt::{
	core::exception::Error as FrameworkError,
	db::{backends::TransactionExecutor, orm::Model},
};
use serde_json::json;
use uuid::Uuid;

fn finish(request: &RequestDigest<'_>) -> Option<Manifest> {
	if request.method != "POST" || request.path != "/transactions/finish" {
		return None;
	}
	let manifest: Manifest = serde_json::from_slice(request.body).ok()?;
	// Reject ignored nested fields as well as changed durable fields.
	let raw: serde_json::Value = serde_json::from_slice(request.body).ok()?;
	(raw == json!(manifest)).then_some(manifest)
}

fn decision(request: &RequestDigest<'_>) -> Option<Uuid> {
	if request.method != "GET" || !request.body.is_empty() {
		return None;
	}
	let id = request
		.path
		.strip_prefix("/transactions/")?
		.strip_suffix("/decision")?;
	let parsed = Uuid::parse_str(id).ok()?;
	(id == parsed.to_string()).then_some(parsed)
}
fn participant(manifest: &Manifest, address: &NodeAddress) -> bool {
	manifest
		.participants
		.iter()
		.any(|p| p.node_id == address.to_string())
}
async fn participant_record(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
) -> Result<Option<AtomicParticipant>> {
	Ok(AtomicParticipant::objects()
		.filter(AtomicParticipant::field_id().eq(id))
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop())
}
async fn coordinator_record(
	tx: &mut dyn TransactionExecutor,
	id: Uuid,
) -> Result<Option<AtomicCoordinator>> {
	Ok(AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(id))
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.pop())
}
/// Admission authenticates the immutable participant; finish still reads the actual
/// coordinator decision before applying anything, including after revocation.
pub(crate) async fn inbound(
	tx: &mut dyn TransactionExecutor,
	receiver: &NodeAddress,
	sender: &NodeAddress,
	subject: &str,
	request: &RequestDigest<'_>,
) -> Result<bool> {
	if !subject.is_empty() {
		return Ok(false);
	}
	if let Some(manifest) = finish(request) {
		let Some(row) = participant_record(tx, manifest.id).await? else {
			return Ok(false);
		};
		return Ok(row.coordinator == sender.to_string()
			&& manifest.coordinator == row.coordinator
			&& row.digest == manifest.digest()?
			&& row.manifest.0 == json!(manifest)
			&& participant(&manifest, receiver));
	}
	if let Some(id) = decision(request) {
		let Some(row) = coordinator_record(tx, id).await? else {
			return Ok(false);
		};
		let manifest: Manifest = serde_json::from_value(row.manifest.0)?;
		return Ok(row.decision.is_some()
			&& manifest.id == id
			&& row.digest == manifest.digest()?
			&& manifest.coordinator == receiver.to_string()
			&& participant(&manifest, sender));
	}
	Ok(false)
}
/// Only a decided coordinator sends finish; an admitted participant may ask its
/// original coordinator for the decision it must learn to discharge its obligation.
pub(crate) async fn outbound(
	tx: &mut dyn TransactionExecutor,
	sender: &NodeAddress,
	receiver: &NodeAddress,
	subject: &str,
	request: &RequestDigest<'_>,
) -> Result<bool> {
	if !subject.is_empty() {
		return Ok(false);
	}
	if let Some(manifest) = finish(request) {
		let Some(row) = coordinator_record(tx, manifest.id).await? else {
			return Ok(false);
		};
		return Ok(row.decision.is_some()
			&& row.digest == manifest.digest()?
			&& row.manifest.0 == json!(manifest)
			&& manifest.coordinator == sender.to_string()
			&& participant(&manifest, receiver));
	}
	if let Some(id) = decision(request) {
		let Some(row) = participant_record(tx, id).await? else {
			return Ok(false);
		};
		let manifest: Manifest = serde_json::from_value(row.manifest.0)?;
		return Ok(manifest.id == id
			&& row.digest == manifest.digest()?
			&& row.coordinator == receiver.to_string()
			&& manifest.coordinator == row.coordinator
			&& participant(&manifest, sender));
	}
	Ok(false)
}
