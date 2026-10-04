use super::{LocalStatus, Manifest, Status, Vote, participant};
use crate::apps::federation::transactions::models::{
	coordinator_records,
	states::{AtomicCoordinatorDecision, AtomicVotePhase},
};
use crate::apps::federation::transactions::services::decisions::CoordinatorTransition;
use crate::{Error, Result, federation::Federation};
use chrono::Utc;
use reinhardt::db::backends::{DatabaseConnection as BackendConnection, dialect::PostgresBackend};
use reinhardt::db::orm::DatabaseConnectionLease;
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub async fn status(f: &Federation, id: Uuid) -> Result<Status> {
	let lease = coordinator_connection(f)?;
	coordinator_records::status(&mut lease.handle(), id).await
}
pub async fn votes(f: &Federation, id: Uuid) -> Result<Vec<Vote>> {
	let lease = coordinator_connection(f)?;
	coordinator_records::votes(&mut lease.handle(), id).await
}
pub async fn submit(f: &Federation, manifest: &Manifest) -> Result<Status> {
	submit_bound(f, manifest, None).await
}
pub(super) async fn submit_bound(
	f: &Federation,
	manifest: &Manifest,
	origin: Option<&super::authority::Origin>,
) -> Result<Status> {
	let db = BackendConnection::new(Arc::new(PostgresBackend::new(f.store.control_pool.clone())));
	let mut tx = db.begin().await?;
	let stored = submit_in(f, manifest, origin, tx.as_mut()).await?;
	super::fault::cut(manifest.id, "coordinator.submit.before").await?;
	tx.commit().await?;
	super::fault::cut(manifest.id, "coordinator.submit.after").await?;
	f.notify.notify_waiters();
	Ok(stored)
}
pub(super) async fn submit_in(
	f: &Federation,
	manifest: &Manifest,
	origin: Option<&super::authority::Origin>,
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
) -> Result<Status> {
	super::validate(manifest)?;
	if manifest.coordinator != f.config.node_id {
		return Err(Error::Invalid("submit to the named coordinator".into()));
	}
	match coordinator_records::status_in(tx, manifest.id).await {
		Ok(existing) => {
			super::authority::match_origin_native(tx, manifest.id, origin).await?;
			if existing.digest != manifest.digest()? || existing.manifest != json!(manifest) {
				return Err(Error::Conflict(
					"transaction ID already has another immutable manifest".into(),
				));
			}
			return Ok(existing);
		}
		Err(Error::NotFound(_)) => {}
		Err(error) => return Err(error),
	}
	let remaining = manifest
		.deadline
		.signed_duration_since(Utc::now())
		.num_seconds();
	if !(1..=3600).contains(&remaining) {
		return Err(Error::Invalid(
			"new transaction deadline must be within the next hour".into(),
		));
	}
	for node in &manifest.participants {
		if node.node_id != f.config.node_id {
			f.peer(&node.node_id).await?;
			if !crate::apps::federation::transactions::models::AtomicPeerTrust::permits(
				tx,
				&node.node_id,
			)
			.await?
			{
				return Err(Error::Forbidden);
			}
		}
	}
	let stored = coordinator_records::admit_in(tx, manifest).await?;
	if let Some(origin) = origin {
		super::authority::bind_native(tx, "atomic_subjects", manifest.id, &json!(origin)).await?;
	}
	super::authority::match_origin_native(tx, manifest.id, origin).await?;
	Ok(stored)
}

pub(crate) async fn remote<T: DeserializeOwned>(
	f: &Federation,
	node: &str,
	method: Method,
	path: &str,
	body: Option<&impl Serialize>,
) -> Result<T> {
	let value = body.map(serde_json::to_value).transpose()?;
	tokio::time::timeout(Duration::from_secs(10), async {
		let response = f.peer_response(node, method, path, value.as_ref()).await?;
		let status = response.status();
		if !status.is_success() {
			return Err(match status {
				reqwest::StatusCode::BAD_REQUEST
				| reqwest::StatusCode::UNPROCESSABLE_ENTITY
				| reqwest::StatusCode::NOT_FOUND
				| reqwest::StatusCode::METHOD_NOT_ALLOWED => Error::Invalid(format!(
					"transaction participant rejected request: {status}"
				)),
				reqwest::StatusCode::UNAUTHORIZED => Error::Unauthorized,
				reqwest::StatusCode::FORBIDDEN => Error::Forbidden,
				reqwest::StatusCode::CONFLICT => Error::Conflict(
					"transaction participant rejected its state precondition".into(),
				),
				reqwest::StatusCode::SERVICE_UNAVAILABLE
					if response
						.headers()
						.get("x-aidash-transaction-pending")
						.is_some_and(|v| v == "1") =>
				{
					Error::TransactionPending
				}
				_ => Error::External(format!("transaction participant returned {status}")),
			});
		}
		crate::response::json(response, 4_194_304).await
	})
	.await
	.map_err(|_| {
		Error::External(
			"transaction participant response timed out; outcome retained for recovery".into(),
		)
	})?
}
pub(crate) async fn decision(f: &Federation, manifest: &Manifest) -> Result<Status> {
	let proof: Status = if manifest.coordinator == f.config.node_id {
		status(f, manifest.id).await?
	} else {
		remote(
			f,
			&manifest.coordinator,
			Method::GET,
			&format!("/transactions/{}/decision", manifest.id),
			None::<&()>,
		)
		.await?
	};
	if proof.id != manifest.id
		|| proof.digest != manifest.digest()?
		|| proof.manifest != json!(manifest)
		|| proof
			.decision
			.as_deref()
			.is_some_and(|d| !matches!(d, "COMMIT" | "ABORT"))
		|| (proof.visible && proof.decision.as_deref() != Some("COMMIT"))
	{
		return Err(Error::Conflict(
			"coordinator decision does not match participant manifest".into(),
		));
	}
	Ok(proof)
}
async fn send(f: &Federation, manifest: &Manifest, node: &str, phase: &str) -> Result<LocalStatus> {
	if phase == "reserve" {
		super::authority::issue(f, manifest, node).await?;
	}
	if node == f.config.node_id {
		match phase {
			"reserve" => participant::reserve(f, &manifest.coordinator, manifest).await,
			"prepare" => participant::prepare(f, &manifest.coordinator, manifest).await,
			_ => participant::finish(f, &manifest.coordinator, manifest).await,
		}
	} else {
		remote(
			f,
			node,
			Method::POST,
			&format!("/transactions/{phase}"),
			Some(manifest),
		)
		.await
	}
}
pub(crate) fn coordinator_connection(f: &Federation) -> Result<DatabaseConnectionLease> {
	Ok(DatabaseConnectionLease::register(BackendConnection::new(
		Arc::new(PostgresBackend::new(f.store.control_pool.clone())),
	))?)
}

async fn record_decision(
	f: &Federation,
	id: Uuid,
	decision: AtomicCoordinatorDecision,
	reason: &str,
) -> Result<()> {
	let lease = coordinator_connection(f)?;
	super::fault::cut(
		id,
		&format!("coordinator.{}.before", decision.as_str().to_lowercase()),
	)
	.await?;
	coordinator_records::transition(
		lease.handle(),
		id,
		CoordinatorTransition::Decide(decision.clone()),
		reason,
	)
	.await?;
	super::fault::cut(
		id,
		&format!("coordinator.{}.after", decision.as_str().to_lowercase()),
	)
	.await?;
	Ok(())
}

pub async fn abort(f: &Federation, id: Uuid) -> Result<Status> {
	// The locked ORM transition arbitrates commit versus abort.
	// Do not require the recovery lease: it spans peer I/O, and contention must
	// not discard an operator's abort. An in-flight transition cannot overwrite
	// this immutable decision; subsequent recovery finalizes the chosen result.
	record_decision(
		f,
		id,
		AtomicCoordinatorDecision::Abort,
		"operator requested abort",
	)
	.await?;
	let existing = status(f, id).await?;
	if existing.decision.as_deref() == Some("COMMIT") {
		return Err(Error::Conflict("commit is irrevocable".into()));
	}
	Ok(existing)
}

/// Make one durable protocol transition. Recovery repeats this exact function;
/// it needs no process-local state or ownership inferred from a timeout.
pub async fn advance(f: &Federation, id: Uuid) -> Result<Status> {
	let backend =
		BackendConnection::new(Arc::new(PostgresBackend::new(f.store.control_pool.clone())));
	let lease = coordinator_records::recovery_lease(&backend, id).await?;
	let result = advance_locked(f, id).await;
	// Release the recovery lease before acknowledging the transition to its caller.
	lease.commit().await?;
	result
}
async fn advance_locked(f: &Federation, id: Uuid) -> Result<Status> {
	let state = status(f, id).await?;
	if state.complete {
		return Ok(state);
	}
	let connection = coordinator_connection(f)?;
	coordinator_records::touch(connection.handle(), id).await?;
	let manifest: Manifest = serde_json::from_value(state.manifest.clone())?;
	if state.decision.is_none() && manifest.deadline <= Utc::now() {
		record_decision(
			f,
			id,
			AtomicCoordinatorDecision::Abort,
			"deadline elapsed before durable decision",
		)
		.await?;
		return status(f, id).await;
	}
	let votes = votes(f, id).await?;
	let selected = if state.decision.is_none() {
		votes
			.iter()
			.find(|v| v.phase == "PENDING")
			.map(|v| (v, "reserve"))
			.or_else(|| {
				votes
					.iter()
					.find(|v| v.phase == "RESERVED")
					.map(|v| (v, "prepare"))
			})
	} else if state.decision.as_deref() == Some("ABORT") {
		votes
			.iter()
			.find(|v| v.phase != "ABORTED")
			.map(|v| (v, "finish"))
	} else if !state.visible {
		votes
			.iter()
			.find(|v| !matches!(v.phase.as_str(), "APPLIED" | "COMMITTED"))
			.map(|v| (v, "finish"))
	} else {
		votes
			.iter()
			.find(|v| v.phase != "COMMITTED")
			.map(|v| (v, "finish"))
	};
	if let Some((vote, operation)) = selected {
		match send(f, &manifest, &vote.node_id, operation).await {
			Ok(result) => {
				if result.id != manifest.id
					|| result.coordinator != manifest.coordinator
					|| result.digest != state.digest
					|| result.manifest != state.manifest
				{
					return Err(Error::Conflict(
						"participant acknowledged another manifest".into(),
					));
				}
				let expected = match operation {
					"reserve" => "RESERVED",
					"prepare" => "PREPARED",
					_ if state.decision.as_deref() == Some("ABORT") => "ABORTED",
					_ if state.visible => "COMMITTED",
					_ => "APPLIED",
				};
				if result.phase != expected {
					return Err(Error::Conflict(
						"participant returned an unexpected phase".into(),
					));
				}
				super::fault::cut(id, "coordinator.vote.before").await?;
				super::authority::settle(f, id, &vote.node_id, &result.phase).await?;
				let phase: AtomicVotePhase = serde_json::from_value(json!(result.phase))?;
				coordinator_records::acknowledge(connection.handle(), id, &vote.node_id, phase)
					.await?;
				super::fault::cut(id, "coordinator.vote.after").await?;
			}
			Err(error) => {
				if state.decision.is_none()
					&& (matches!(
						error,
						Error::Conflict(_)
							| Error::Invalid(_) | Error::NotFound(_)
							| Error::Forbidden | Error::Unauthorized
					) || (matches!(error, Error::TransactionPending)
						&& super::authority::scoped(f, id).await?))
				{
					record_decision(f, id, AtomicCoordinatorDecision::Abort, &error.to_string())
						.await?;
				} else {
					coordinator_records::record_error(connection.handle(), id, &error.to_string())
						.await?;
				}
			}
		}
	} else if state.decision.is_none() {
		if !votes.iter().all(|v| v.phase == "PREPARED") {
			return Err(Error::Conflict(
				"commit requires every prepared vote".into(),
			));
		}
		record_decision(f, id, AtomicCoordinatorDecision::Commit, "").await?;
	} else {
		let lease = coordinator_connection(f)?;
		let (change, detail) = if state.decision.as_deref() == Some("COMMIT") && !state.visible {
			(
				CoordinatorTransition::Publish,
				"every participant durably applied commit",
			)
		} else {
			(
				CoordinatorTransition::Complete,
				"every participant finalized",
			)
		};
		let point = if state.decision.as_deref() == Some("COMMIT") && !state.visible {
			"coordinator.visible"
		} else {
			"coordinator.complete"
		};
		super::fault::cut(id, &format!("{point}.before")).await?;
		coordinator_records::transition(lease.handle(), id, change, detail).await?;
		super::fault::cut(id, &format!("{point}.after")).await?;
	}
	status(f, id).await
}
pub async fn recover_once(f: &Federation) -> Result<()> {
	recover_kind(f, false).await?;
	recover_kind(f, true).await
}
async fn recover_kind(f: &Federation, aborted: bool) -> Result<()> {
	let connection = coordinator_connection(f)?;
	let ids = coordinator_records::recovery_candidates(&mut connection.handle(), aborted).await?;
	for id in ids {
		if let Err(error) = advance(f, id).await {
			tracing::warn!(%id,%error,"atomic transaction recovery pending");
		}
	}
	Ok(())
}
pub async fn run(f: Federation) -> Result<()> {
	// Aborted history may have unreachable peers indefinitely. Its network
	// waits must not occupy the loop or connection capacity for active work.
	let aborted = f.for_recovery().await?;
	async fn recover(f: Federation, aborted: bool) -> Result<()> {
		loop {
			if let Err(error) = recover_kind(&f, aborted).await {
				tracing::warn!(%error,aborted,"coordinator recovery failed; decisions retained");
			}
			tokio::time::sleep(Duration::from_millis(if aborted { 1000 } else { 100 })).await;
		}
	}
	tokio::try_join!(recover(f, false), recover(aborted, true))?;
	Ok(())
}

pub(super) async fn abort_in(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	id: Uuid,
) -> Result<()> {
	coordinator_records::transition_in(
		tx,
		id,
		CoordinatorTransition::Decide(AtomicCoordinatorDecision::Abort),
		"subject requested abort",
	)
	.await?;
	if coordinator_records::status_in(tx, id)
		.await?
		.decision
		.as_deref()
		== Some("COMMIT")
	{
		return Err(Error::Conflict("commit is irrevocable".into()));
	}
	super::fault::cut(id, "coordinator.abort.before").await
}
