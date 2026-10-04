use super::{Manifest, Status, Vote};
use crate::apps::federation::transactions::models::{
	coordinator_records, states::AtomicCoordinatorDecision,
};
pub(crate) use crate::apps::federation::transactions::repositories::coordination::connection as coordinator_connection;
use crate::apps::federation::transactions::services::decisions::CoordinatorTransition;
use crate::{Error, Result, federation::Federation};
use chrono::Utc;
use reinhardt::db::backends::{DatabaseConnection as BackendConnection, dialect::PostgresBackend};
use reqwest::Method;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub async fn status(f: &Federation, id: Uuid) -> Result<Status> {
	crate::bootstrap::transaction_coordinator(f)
		.status(id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn votes(f: &Federation, id: Uuid) -> Result<Vec<Vote>> {
	crate::bootstrap::transaction_coordinator(f)
		.votes(id)
		.await
		.map(|votes| votes.into_iter().map(Into::into).collect())
		.map_err(Into::into)
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
pub(crate) async fn submit_in(
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
	crate::bootstrap::transaction_coordinator(f)
		.decision(manifest)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn abort(f: &Federation, id: Uuid) -> Result<Status> {
	crate::bootstrap::transaction_coordinator(f)
		.abort(id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
/// Make one durable protocol transition using the same application workflow as recovery.
pub async fn advance(f: &Federation, id: Uuid) -> Result<Status> {
	crate::bootstrap::transaction_coordinator(f)
		.advance(id)
		.await
		.map(Into::into)
		.map_err(Into::into)
}
pub async fn recover_once(f: &Federation) -> Result<()> {
	crate::bootstrap::transaction_coordinator(f)
		.recover_once()
		.await
		.map_err(Into::into)
}
async fn recover_kind(f: &Federation, aborted: bool) -> Result<()> {
	crate::bootstrap::transaction_coordinator(f)
		.recover_kind(aborted)
		.await
		.map_err(Into::into)
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

pub(crate) async fn abort_in(
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
