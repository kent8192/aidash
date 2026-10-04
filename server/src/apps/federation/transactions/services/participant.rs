//! Durable participant coordination over native serializable transactions.
use super::{LocalStatus, Manifest, coordinator, mutation};
use crate::apps::federation::transactions::models::{
	AtomicGate, AtomicHistory, AtomicParticipant, AtomicPeerTrust,
};
use crate::apps::federation::transactions::services::states::AtomicParticipantPhase;
use crate::{Error, Result, federation::Federation};
use reinhardt::db::backends::{DatabaseConnection, PostgresBackend, TransactionExecutor};
use serde_json::json;
use std::sync::Arc;

async fn begin(f: &Federation) -> Result<Box<dyn TransactionExecutor>> {
	let db = DatabaseConnection::new(Arc::new(PostgresBackend::new(f.store.control_pool.clone())));
	AtomicParticipant::begin(&db).await
}

fn check(existing: &AtomicParticipant, manifest: &Manifest) -> Result<()> {
	if existing.coordinator != manifest.coordinator
		|| existing.digest != manifest.digest()?
		|| existing.manifest.0 != json!(manifest)
	{
		return Err(Error::Conflict(
			"transaction ID already has another immutable manifest".into(),
		));
	}
	Ok(())
}

fn sender(f: &Federation, caller: &str, manifest: &Manifest) -> Result<()> {
	super::validate(manifest)?;
	manifest.local(&f.config.node_id)?;
	if caller != manifest.coordinator {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub async fn reserve(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	sender(f, caller, manifest)?;
	// Durable admission is checked before live authority: recovery cannot be
	// cancelled by revoking a grant which already created an obligation.
	let mut tx = begin(f).await?;
	if let Some(existing) = AtomicParticipant::lock(tx.as_mut(), manifest.id).await? {
		check(&existing, manifest)?;
		tx.commit().await?;
		return Ok(existing.into());
	}
	tx.rollback().await?;
	if let Some(access) = super::authority::admission(f, caller, manifest).await? {
		let mut access = access.into_native()?;
		let result = reserve_in(f, caller, manifest, access.tx.as_mut()).await;
		super::fault::cut(manifest.id, "participant.reserve.before").await?;
		let row = access.finish(result).await?;
		super::fault::cut(manifest.id, "participant.reserve.after").await?;
		Ok(row)
	} else {
		let mut tx = begin(f).await?;
		let row = reserve_in(f, caller, manifest, tx.as_mut()).await?;
		super::fault::cut(manifest.id, "participant.reserve.before").await?;
		tx.commit().await?;
		super::fault::cut(manifest.id, "participant.reserve.after").await?;
		Ok(row)
	}
}
async fn reserve_in(
	f: &Federation,
	caller: &str,
	manifest: &Manifest,
	tx: &mut dyn TransactionExecutor,
) -> Result<LocalStatus> {
	if let Some(existing) = AtomicParticipant::lock(tx, manifest.id).await? {
		check(&existing, manifest)?;
		return Ok(existing.into());
	}
	if caller != f.config.node_id && !AtomicPeerTrust::permits(tx, caller).await? {
		return Err(Error::Forbidden);
	}
	if AtomicGate::lock_exclusive(tx).await?.is_some() {
		return Err(Error::TransactionPending);
	}
	let row = AtomicParticipant::insert(tx, manifest, AtomicParticipantPhase::Reserved).await?;
	AtomicGate::reserve(tx, manifest.id).await?;
	AtomicHistory::append(
		tx,
		manifest.id,
		"participant",
		"RESERVED",
		"node visibility barrier persisted",
	)
	.await?;
	Ok(row.into())
}

pub async fn prepare(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	sender(f, caller, manifest)?;
	let mut tx = begin(f).await?;
	let existing = AtomicParticipant::lock(tx.as_mut(), manifest.id)
		.await?
		.ok_or_else(|| Error::Conflict("participant was not reserved".into()))?;
	check(&existing, manifest)?;
	if existing.phase != AtomicParticipantPhase::Reserved {
		tx.commit().await?;
		return Ok(existing.into());
	}
	if AtomicGate::lock_exclusive(tx.as_mut()).await? != Some(manifest.id) {
		return Err(Error::Conflict(
			"participant lost its visibility barrier".into(),
		));
	}
	AtomicParticipant::mutation_context(tx.as_mut(), manifest.id).await?;
	tx.savepoint("participant_validation").await?;
	mutation::apply(&f.store, tx.as_mut(), manifest).await?;
	tx.rollback_to_savepoint("participant_validation").await?;
	tx.release_savepoint("participant_validation").await?;
	let row =
		AtomicParticipant::transition(tx.as_mut(), manifest.id, AtomicParticipantPhase::Prepared)
			.await?;
	super::fault::cut(manifest.id, "participant.prepare.before").await?;
	tx.commit().await?;
	super::fault::cut(manifest.id, "participant.prepare.after").await?;
	Ok(row.into())
}

pub async fn finish(f: &Federation, caller: &str, manifest: &Manifest) -> Result<LocalStatus> {
	sender(f, caller, manifest)?;
	// A message is only a wake-up. Its sender cannot supply the decision.
	let proof = coordinator::decision(f, manifest).await?;
	let Some(decision) = proof.decision.as_deref() else {
		return Err(Error::TransactionPending);
	};
	let mut tx = begin(f).await?;
	let existing = AtomicParticipant::lock(tx.as_mut(), manifest.id).await?;
	if let Some(existing) = &existing {
		check(existing, manifest)?;
	}
	if decision == "ABORT" {
		if let Some(existing) = existing {
			if matches!(
				existing.phase,
				AtomicParticipantPhase::Applied | AtomicParticipantPhase::Committed
			) {
				return Err(Error::Conflict("commit cannot be aborted".into()));
			}
			if existing.phase == AtomicParticipantPhase::Aborted {
				tx.commit().await?;
				return Ok(existing.into());
			}
			if AtomicGate::lock_exclusive(tx.as_mut()).await? != Some(manifest.id) {
				return Err(Error::Conflict(
					"participant lost its visibility barrier".into(),
				));
			}
			AtomicGate::release(tx.as_mut(), false).await?;
		} else {
			// Abort tombstones prevent a delayed reserve from resurrecting work.
			AtomicParticipant::insert(tx.as_mut(), manifest, AtomicParticipantPhase::Aborted)
				.await?;
		}
		let row = AtomicParticipant::transition(
			tx.as_mut(),
			manifest.id,
			AtomicParticipantPhase::Aborted,
		)
		.await?;
		super::fault::cut(manifest.id, "participant.abort.before").await?;
		tx.commit().await?;
		super::fault::cut(manifest.id, "participant.abort.after").await?;
		return Ok(row.into());
	}
	let existing =
		existing.ok_or_else(|| Error::Conflict("commit requires a prepared participant".into()))?;
	if existing.phase == AtomicParticipantPhase::Committed {
		tx.commit().await?;
		return Ok(existing.into());
	}
	if !matches!(
		existing.phase,
		AtomicParticipantPhase::Prepared | AtomicParticipantPhase::Applied
	) {
		return Err(Error::Conflict(
			"commit requires a prepared participant".into(),
		));
	}
	if AtomicGate::lock_exclusive(tx.as_mut()).await? != Some(manifest.id) {
		return Err(Error::Conflict(
			"participant lost its visibility barrier".into(),
		));
	}
	if existing.phase == AtomicParticipantPhase::Prepared {
		AtomicParticipant::mutation_context(tx.as_mut(), manifest.id).await?;
		mutation::apply(&f.store, tx.as_mut(), manifest).await?;
		AtomicParticipant::transition(tx.as_mut(), manifest.id, AtomicParticipantPhase::Applied)
			.await?;
	}
	let row = if proof.visible {
		AtomicGate::release(tx.as_mut(), true).await?;
		AtomicParticipant::transition(tx.as_mut(), manifest.id, AtomicParticipantPhase::Committed)
			.await?
	} else {
		AtomicParticipant::lock(tx.as_mut(), manifest.id)
			.await?
			.ok_or_else(|| Error::Conflict("participant disappeared during commit".into()))?
	};
	let point = if proof.visible {
		"participant.release"
	} else {
		"participant.apply"
	};
	super::fault::cut(manifest.id, &format!("{point}.before")).await?;
	tx.commit().await?;
	super::fault::cut(manifest.id, &format!("{point}.after")).await?;
	f.notify.notify_waiters();
	Ok(row.into())
}

pub async fn recover_once(f: &Federation) -> Result<usize> {
	let lease = coordinator::coordinator_connection(f)?;
	let pending = AtomicParticipant::pending(&mut lease.handle()).await?;
	let mut completed = 0;
	for row in pending {
		let manifest: Manifest = serde_json::from_value(row.manifest.into_inner())?;
		// No timeout can decide an outcome. Fetching a durable decision is the
		// only way a participant can make progress without a coordinator push.
		match coordinator::decision(f, &manifest).await {
			Ok(proof) if proof.decision.is_some() => {
				match finish(f, &manifest.coordinator, &manifest).await {
					Ok(_) => completed += 1,
					Err(error) => {
						tracing::debug!(id=%manifest.id,%error,"participant recovery waits")
					}
				}
			}
			Ok(_) => {}
			Err(error) => {
				tracing::debug!(id=%manifest.id,%error,"participant decision unavailable")
			}
		}
	}
	Ok(completed)
}

pub async fn run(f: Federation) -> Result<()> {
	loop {
		if let Err(error) = recover_once(&f).await {
			tracing::warn!(%error,"participant recovery failed; barriers retained");
		}
		tokio::time::sleep(std::time::Duration::from_secs(1)).await;
	}
}
