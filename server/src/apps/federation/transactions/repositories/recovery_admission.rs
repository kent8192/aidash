//! Immutable durable obligations, not a route prefix or caller-selected recovery flag.
use crate::{
	Result,
	apps::federation::transactions::models::{AtomicCoordinator, AtomicParticipant},
};
use aidash_application::federation::inbound::RequestDigest;
use aidash_domain::{
	nodes::NodeAddress,
	transactions::{Manifest, share::Share},
};
use reinhardt::{
	core::exception::Error as FrameworkError,
	db::{backends::TransactionExecutor, orm::Model},
};
use serde_json::json;
use uuid::Uuid;

/// A finish request carries the receiving participant's own share (ADR 0146).
fn finish(request: &RequestDigest<'_>) -> Option<Share> {
	if request.method != "POST" || request.path != "/transactions/finish" {
		return None;
	}
	let share: Share = serde_json::from_slice(request.body).ok()?;
	// Reject ignored nested fields as well as changed durable fields.
	let raw: serde_json::Value = serde_json::from_slice(request.body).ok()?;
	(raw == json!(share)).then_some(share)
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
fn participant(nodes: &[String], address: &NodeAddress) -> bool {
	nodes.iter().any(|node| *node == address.to_string())
}
fn nodes(manifest: &Manifest) -> Vec<String> {
	manifest
		.participants
		.iter()
		.map(|participant| participant.node_id.clone())
		.collect()
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
	if let Some(share) = finish(request) {
		let Some(row) = participant_record(tx, share.id).await? else {
			return Ok(false);
		};
		return Ok(row.coordinator == sender.to_string()
			&& share.coordinator == row.coordinator
			&& row.digest == share.digest()?
			&& row.manifest.0 == json!(share)
			&& share.part.node_id == receiver.to_string());
	}
	if let Some(id) = decision(request) {
		let Some(row) = coordinator_record(tx, id).await? else {
			return Ok(false);
		};
		let manifest: Manifest = serde_json::from_value(row.manifest.0)?;
		return Ok(row.decision.is_some()
			&& manifest.id == id
			&& row.digest == manifest.sealed_digest(&row.seed)?
			&& manifest.coordinator == receiver.to_string()
			&& participant(&nodes(&manifest), sender));
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
	if let Some(share) = finish(request) {
		let Some(row) = coordinator_record(tx, share.id).await? else {
			return Ok(false);
		};
		let manifest: Manifest = serde_json::from_value(row.manifest.0.clone())?;
		return Ok(row.decision.is_some()
			&& row.digest == share.digest()?
			&& share.part.node_id == receiver.to_string()
			&& json!(share) == json!(manifest.share(&row.seed, &share.part.node_id)?)
			&& share.coordinator == sender.to_string());
	}
	if let Some(id) = decision(request) {
		let Some(row) = participant_record(tx, id).await? else {
			return Ok(false);
		};
		let share: Share = serde_json::from_value(row.manifest.0)?;
		return Ok(share.id == id
			&& row.digest == share.digest()?
			&& row.coordinator == receiver.to_string()
			&& share.coordinator == row.coordinator
			&& share.part.node_id == sender.to_string());
	}
	Ok(false)
}
