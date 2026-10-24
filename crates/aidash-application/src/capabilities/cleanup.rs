//! Explicit retention, deletion and restoration share current authority and owned transactions.
use crate::{
	Error, Result,
	ports::capabilities::cleanup::{CleanupRepository, CleanupScope, Creation},
};
use aidash_domain::capabilities::{
	cleanup::*,
	operations::{FileScope, MountedFile as FileEntry},
	records::Record,
	sessions::Area,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
fn result(record: &Record) -> Result<CleanupResult> {
	aidash_domain::capabilities::cleanup::result(record).ok_or(Error::Forbidden)
}
fn files(area: &Area) -> Result<Vec<FileEntry>> {
	Ok(serde_json::from_value(area.manifest.clone())?)
}
pub async fn load(scope: &mut dyn CleanupScope, id: Uuid, action: &str) -> Result<Area> {
	let area: Area = scope
		.load_area(id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	let admin = scope
		.bundle()
		.subjects
		.get(scope.principal())
		.is_some_and(|s| s.enabled && s.attributes["file_administrator"] == true);
	if area.owner != scope.principal() && !admin {
		return Err(Error::NotFound("working area unavailable".into()));
	}
	let workspace = scope.workspace(area.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	scope.require(&workspace, "workspace.read").await?;
	scope
		.require(
			&scope.resource(
				"working_area",
				id,
				json!({"owner":area.owner,"agent_id":area.agent_id,"thread_id":area.thread_id}),
			),
			action,
		)
		.await?;
	Ok(area)
}
pub async fn inventory(
	scope: &mut dyn CleanupScope,
	cursor: Option<Uuid>,
) -> Result<ManagementPage> {
	let rows: Vec<Area> = scope.inventory(cursor).await?;
	let next_cursor = (rows.len() == 51).then(|| rows[49].id);
	let mut items = vec![];
	for row in rows.into_iter().take(50) {
		let area = match load(scope, row.id, "file.manage").await {
			Ok(a) => a,
			Err(Error::NotFound(_) | Error::Forbidden) => continue,
			Err(e) => return Err(e),
		};
		let recovery: Option<Record> = scope.inventory_recovery(&area).await?;
		let files = files(&area)?
			.into_iter()
			.filter(|f| !matches!(f.scope, FileScope::References))
			.collect::<Vec<_>>();
		let cleanup_operation_id = scope.cleanup_operation(&area).await?;
		items.push(ManagedArea {
			cleanup_operation_id,
			area_id: area.id,
			workspace_id: area.workspace_id,
			thread_id: area.thread_id,
			agent_id: area.agent_id,
			owner: area.owner,
			state: area.state,
			revision: area.revision,
			generation: area.generation,
			files: files.len(),
			bytes: files.iter().map(|f| f.size).sum(),
			snapshot_id: recovery.as_ref().map(|r| r.id),
			recovery_expires_at: recovery.and_then(|r| r.expires_at),
		});
	}
	Ok(ManagementPage { items, next_cursor })
}
pub async fn confirmation(scope: &mut dyn CleanupScope, id: Uuid, revision: i64) -> Result<Value> {
	let area = load(scope, id, "file.delete").await?;
	if area.revision != revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	let id = Uuid::new_v4();
	let expiry = Utc::now() + Duration::minutes(10);
	scope
		.create(Creation {
			id,
			area: Some(area.id),
			kind: "deletion_confirmation",
			state: "pending",
			data: json!({"revision":revision,"generation":area.generation}),
			expires: Some(expiry),
		})
		.await?;
	Ok(
		json!({"confirmation_id":id,"area_id":area.id,"revision":revision,"expires_at":expiry,"irreversible":true}),
	)
}
pub async fn prepare(
	scope: &mut dyn CleanupScope,
	id: Uuid,
	input: Cleanup,
) -> Result<CleanupResult> {
	scope.serialize().await?;
	let mut area = load(scope, id, "file.manage").await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["cleanup", id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		return result(
			&scope
				.load_record(
					serde_json::from_value(cached["operation_id"].clone())?,
					"cleanup",
				)
				.await?,
		);
	}
	if area.revision != input.expected_revision {
		return Err(Error::Conflict("AREA_REVISION_CHANGED".into()));
	}
	if matches!(input.choice, Choice::Irreversible) {
		scope
			.require(
				&scope.resource("working_area", area.id, json!({"owner":area.owner})),
				"file.delete",
			)
			.await?;
		let mut confirmation = scope
			.load_record(
				input.confirmation_id.ok_or_else(|| {
					Error::Invalid("SEPARATE_DELETION_CONFIRMATION_REQUIRED".into())
				})?,
				"deletion_confirmation",
			)
			.await?;
		if confirmation.area_id != Some(area.id)
			|| confirmation.owner != scope.principal()
			|| confirmation.state != "pending"
			|| confirmation.data["revision"] != area.revision
			|| confirmation.data["generation"] != area.generation
			|| confirmation.expires_at.is_none_or(|t| t <= Utc::now())
		{
			return Err(Error::Conflict("DELETION_CONFIRMATION_EXPIRED".into()));
		}
		confirmation.state = "used".into();
		scope.update(&mut confirmation).await?;
	} else if input.confirmation_id.is_some() {
		return Err(Error::Invalid("UNEXPECTED_CONFIRMATION".into()));
	}
	if matches!(input.choice, Choice::Keep) && area.state == "recoverable" {
		let mut recovery: Record = scope
			.recoverable(&area)
			.await?
			.ok_or_else(|| Error::Conflict("RECOVERY_UNAVAILABLE".into()))?;
		if recovery.expires_at.is_none_or(|t| t <= Utc::now()) {
			return Err(Error::Conflict("RECOVERY_UNAVAILABLE".into()));
		}
		// Retain the already verified snapshot itself. The expiry worker locks
		// this same area and record, so it cannot erase the bytes after Keep.
		area.manifest = recovery.data["snapshot"].clone();
		area.state = "retained".into();
		area.generation += 1;
		area.epoch += 1;
		scope.publish(&mut area).await?;
		scope.persist(&area, false).await?;
		recovery.state = "kept".into();
		recovery.expires_at = None;
		recovery.data["retained"] = json!(true);
		recovery.data["area_revision"] = json!(area.revision);
		recovery.data["generation"] = json!(area.generation);
		scope.update(&mut recovery).await?;
		scope
			.cache(
				input.idempotency_key,
				&digest,
				&json!({"operation_id":recovery.id}),
			)
			.await?;
		return result(&recovery);
	}
	if !matches!(input.choice, Choice::Keep) {
		if !matches!(
			area.state.as_str(),
			"active" | "retained" | "recoverable" | "deleted"
		) {
			return Err(Error::Conflict(
				"AREA_BUSY: cancel and reconcile the active writer before cleanup".into(),
			));
		}
		scope.release_python(&area, "cleanup").await?;
	}
	let operation = Uuid::new_v4();
	let mut snapshot = vec![];
	if matches!(input.choice, Choice::Recoverable) {
		if !matches!(area.state.as_str(), "active" | "retained") {
			return Err(Error::Conflict("AREA_UNAVAILABLE".into()));
		}
		scope.sources(area.workspace_id, &area.constraints).await?;
		for file in files(&area)? {
			if matches!(file.scope, FileScope::References) {
				snapshot.push(file);
			} else {
				snapshot.push(scope.copy_owned(area.id, "recovery", &file).await?);
			}
		}
	}
	let kept = matches!(input.choice, Choice::Keep);
	let recovery_seconds = scope.limits()?.recovery_seconds;
	let expires = matches!(input.choice, Choice::Recoverable)
		.then(|| Utc::now() + Duration::seconds(recovery_seconds as i64));
	if !kept {
		area.epoch += 1;
		area.generation += 1;
		area.state = "cleaning".into();
		area.manifest = json!(
			files(&area)?
				.into_iter()
				.filter(|f| matches!(f.scope, FileScope::References))
				.collect::<Vec<_>>()
		);
		scope.publish(&mut area).await?;
		scope.persist(&area, false).await?;
		scope.cancel_runs(&area).await?;
		scope.revoke_grants(&area).await?;
	}
	let record=scope.create(Creation {id:operation,area:Some(area.id),kind:"cleanup",state:if kept{"kept"}else{"deleting"},data:json!({"choice":input.choice,"snapshot":snapshot,"constraints":area.constraints,"area_revision":area.revision,"generation":area.generation,"credential_id":scope.credential()}),expires}).await?;
	scope
		.cache(
			input.idempotency_key,
			&digest,
			&json!({"operation_id":operation}),
		)
		.await?;
	result(&record)
}
pub async fn status(scope: &mut dyn CleanupScope, id: Uuid) -> Result<CleanupResult> {
	// The cleanup worker and restore path lock the area before its record.
	// Discover the immutable association without locking, then use that order
	// here too so status polling cannot deadlock the worker completing cleanup.
	let area_id: Option<Uuid> = scope
		.record_area(id)
		.await?
		.ok_or_else(|| Error::NotFound("resource unavailable".into()))?;
	let area_id = area_id.ok_or(Error::Forbidden)?;
	load(scope, area_id, "file.manage").await?;
	let record = scope.load_record(id, "cleanup").await?;
	if record.area_id != Some(area_id) {
		return Err(Error::Forbidden);
	}
	result(&record)
}
pub async fn restore(scope: &mut dyn CleanupScope, id: Uuid, input: Restore) -> Result<Area> {
	scope.serialize().await?;
	let mut area = load(scope, id, "file.restore").await?;
	let digest = aidash_domain::registry::rules::digest(&json!(["restore", id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
		let restored: Area = serde_json::from_value(cached)?;
		scope.sources(area.workspace_id, &area.constraints).await?;
		if area.state != "active" || area.generation != restored.generation {
			return Err(Error::Conflict("RESTORED_GENERATION_CHANGED".into()));
		}
		return Ok(area);
	}
	if area.revision != input.expected_revision
		|| !matches!(area.state.as_str(), "recoverable" | "retained")
	{
		return Err(Error::Conflict("AREA_REVISION_OR_STATE_CHANGED".into()));
	}
	let mut record = scope.load_record(input.snapshot_id, "cleanup").await?;
	let retained =
		record.state == "kept" && record.data["retained"] == true && area.state == "retained";
	if record.area_id != Some(area.id)
		|| !(record.state == "recoverable" || retained)
		|| (!retained && record.expires_at.is_none_or(|t| t <= Utc::now()))
	{
		return Err(Error::Conflict("RECOVERY_UNAVAILABLE".into()));
	}
	scope
		.sources(area.workspace_id, &record.data["constraints"])
		.await?;
	scope.visible_thread(input.thread_id).await?;
	if input.thread_id != area.thread_id {
		let root: Option<Uuid> = scope.thread_root(&area, input.thread_id).await?;
		scope
			.message(
				area.workspace_id,
				root.ok_or_else(|| Error::NotFound("restoration thread unavailable".into()))?,
			)
			.await?;
	}
	let occupied: Option<Uuid> = scope.occupied(&area, input.thread_id).await?;
	if occupied.is_some() {
		return Err(Error::Conflict("RESTORATION_THREAD_OCCUPIED".into()));
	}
	let snapshot: Vec<FileEntry> = serde_json::from_value(record.data["snapshot"].clone())?;
	let bytes = snapshot
		.iter()
		.filter(|file| !matches!(file.scope, FileScope::References))
		.try_fold(0_u64, |total, file| total.checked_add(file.size));
	let working_bytes = scope.limits()?.working_bytes;
	if bytes.is_none_or(|bytes| bytes > working_bytes) {
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let mut files = vec![];
	for file in snapshot {
		if matches!(file.scope, FileScope::References) {
			// References are mounted afresh from the selected immutable Agent
			// version at admission; restoring files cannot reinstate old mounts.
			continue;
		} else if retained {
			scope.verified(&file).await?;
			if matches!(file.scope, FileScope::Working) {
				// The snapshot is consumed by this transaction. Its reattached
				// working bytes must follow normal replacement/reclamation rules.
				scope.reattach_working(&area, file.file_id).await?;
			}
			files.push(file);
		} else {
			files.push(scope.copy_owned(area.id, "working", &file).await?);
		}
	}
	area.manifest = json!(files);
	area.constraints = record.data["constraints"].clone();
	area.generation += 1;
	area.epoch += 1;
	area.thread_id = input.thread_id;
	area.state = "active".into();
	scope.publish(&mut area).await?;
	scope.persist(&area, true).await?;
	record.state = if retained { "reattached" } else { "restored" }.into();
	record.expires_at = (!retained).then(Utc::now);
	if retained {
		record.data["snapshot"] = json!([]);
	}
	scope.update(&mut record).await?;
	scope
		.cache(input.idempotency_key, &digest, &json!(area))
		.await?;
	Ok(area)
}
pub async fn erase_job(repository: &dyn CleanupRepository, snapshot: Record) -> Result<()> {
	let mut scope = repository.begin().await?;
	let result = async {
		let (mut area, mut record) = scope.locked(&snapshot).await?;

		let expiring = matches!(record.state.as_str(), "restored" | "recoverable")
			&& record.expires_at.is_some_and(|t| t <= Utc::now());
		if !expiring && !matches!(record.state.as_str(), "deleting" | "cleanup_failed") {
			return Ok(false);
		}
		let snapshot_files: Vec<FileEntry> =
			serde_json::from_value(record.data["snapshot"].clone())?;
		let snapshot_ids = snapshot_files
			.iter()
			.filter(|f| !matches!(f.scope, FileScope::References))
			.map(|f| f.file_id)
			.collect::<std::collections::BTreeSet<_>>();
		if expiring {
			if record.state == "recoverable"
				&& (area.state != "recoverable" || record.data["generation"] != area.generation)
			{
				return Err(Error::Conflict("CLEANUP_GENERATION_CHANGED".into()));
			}
			for id in snapshot_ids {
				scope.erase(&area.tenant, id).await?;
			}
			if record.state == "recoverable" {
				area.state = "deleted".into();
				area.generation += 1;
				area.epoch += 1;
				area.revision += 1;
			}
			record.state = "expired".into();
			record.data["snapshot"] = json!([]);
			record.expires_at = None;
		} else {
			if area.generation != record.data["generation"]
				|| !matches!(area.state.as_str(), "cleaning" | "cleanup_failed")
			{
				return Err(Error::Conflict("CLEANUP_GENERATION_CHANGED".into()));
			}
			let ids: Vec<Uuid> = scope.objects(&area).await?;
			for id in ids {
				if !snapshot_ids.contains(&id) {
					scope.erase(&area.tenant, id).await?;
				}
			}
			record.state = if record.data["choice"] == "recoverable" {
				"recoverable"
			} else {
				"deleted"
			}
			.into();
			area.state = record.state.clone();
		}
		scope.update_area(&area).await?;
		scope.update_record(&mut record).await?;
		scope
			.event(
				area.workspace_id,
				"capability.cleanup_changed",
				json!({"area_id":area.id,"operation_id":record.id,"status":record.state}),
			)
			.await?;
		Ok(true)
	}
	.await;
	scope.finish(result).await
}
pub async fn sweep(repository: &dyn CleanupRepository, cursor: &mut Uuid) -> Result<()> {
	let jobs = repository.jobs(*cursor).await?;
	if jobs.is_empty() {
		*cursor = Uuid::nil();
	}
	for job in jobs {
		let id = job.id;
		*cursor = id;
		let area = job.area_id;
		if let Err(error) = Box::pin(erase_job(repository, job)).await {
			repository.failure(id, area).await?;
			tracing::warn!(%error,"file lifecycle reconciliation pending");
		}
	}
	Ok(())
}
#[cfg(test)]
mod tests;
