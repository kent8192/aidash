//! A reference retains the File version it names; releasing it lets reclamation resume.
use crate::{
	Error, Result,
	ports::capabilities::retention::{RetentionLimits, RetentionScope},
};
use aidash_domain::capabilities::{
	operations::MountedFile,
	retention::{
		Holder, RetainedFile, RetentionHold, area_permits_retention, permits_retention,
		retained_bytes, retained_files,
	},
	sessions::Area,
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

/// Current authority over the Working Area whose File version is cited.
///
/// Reading the bytes and the held manifest still runs the full read
/// authorization, including source constraints; this check only admits the
/// reference itself.
async fn authorize(scope: &mut dyn RetentionScope, area: &Area) -> Result<()> {
	let workspace = scope.workspace(area.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	if area.tenant != scope.tenant() || area.owner != scope.principal() {
		return Err(Error::NotFound("working area unavailable".into()));
	}
	let resource = scope.resource(
		"working_area",
		area.id,
		json!({"owner":area.owner,"agent_id":area.agent_id,"thread_id":area.thread_id}),
	);
	scope.require(&workspace, "workspace.read").await?;
	scope
		.require(&resource, "file.read")
		.await
		.map_err(|_| Error::NotFound("working area unavailable".into()))
}

fn quota(limits: &RetentionLimits) -> (u64, u64) {
	(limits.working_bytes, limits.retained_bytes)
}

/// Retains the File version `file_id` of `area_id` for `holder`.
///
/// The version must be in the Working Area's manifest at this moment, or a hold
/// that still covers it must exist; a version already reclaimed cannot be
/// resurrected. An HTML version retains every File version of its manifest at
/// that revision so that the page's relative references resolve.
///
/// Idempotent per `(holder, file_id)`: citing the same version twice returns the
/// same hold and writes nothing. Refuses with `QUOTA_EXCEEDED` when keeping the
/// version would exceed the Working Area's byte budget or the Tenant's storage
/// ceiling, where already-reclaimable superseded bytes are discounted.
pub async fn retain(
	scope: &mut dyn RetentionScope,
	area_id: Uuid,
	file_id: Uuid,
	holder: Holder,
) -> Result<RetentionHold> {
	let area = scope
		.area(area_id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(scope, &area).await?;
	let manifest: Vec<MountedFile> = serde_json::from_value(area.manifest.clone())?;
	let holds = scope.holds(area_id).await?;
	let now = Utc::now();
	if let Some(existing) = holds
		.iter()
		.find(|hold| hold.holder.same(&holder) && !hold.holder.expired(now) && hold.covers(file_id))
	{
		return Ok(existing.clone());
	}
	// A version that already left the manifest can still be cited while another
	// hold keeps it, sharing that hold's revision and File version set. Otherwise
	// it is gone and a reference must say so instead of naming another version.
	let (revision, files) =
		if let Some(entry) = manifest.iter().find(|file| file.file_id == file_id) {
			(area.revision, retained_files(entry, &manifest))
		} else if let Some(existing) = holds
			.iter()
			.find(|hold| !hold.holder.expired(now) && hold.covers(file_id))
		{
			(existing.revision, existing.files.clone())
		} else {
			return Err(Error::NotFound("file version is no longer retained".into()));
		};
	let limits = scope.limits()?;
	let (working_bytes, retained_limit) = quota(&limits);
	let current = manifest.iter().map(|file| file.size).sum::<u64>();
	let retained = retained_bytes(&holds, &manifest, now);
	if !area_permits_retention(current, retained, working_bytes) {
		return Err(Error::Conflict(
			"QUOTA_EXCEEDED: retaining this File version would exceed the Working Area's byte limit"
				.into(),
		));
	}
	// Retained versions stay in `core_quotas.used_bytes` until they are erased, so
	// only superseded bytes that no hold covers are reclaimable from the ceiling.
	let used = scope.used_bytes().await?;
	let reclaimable = scope.reclaimable_bytes().await?;
	if !permits_retention(used, reclaimable, retained_limit) {
		return Err(Error::Conflict(
			"QUOTA_EXCEEDED: the Tenant's storage cannot keep this File version".into(),
		));
	}
	let hold = RetentionHold {
		tenant: scope.tenant().to_owned(),
		area_id,
		revision,
		holder,
		files,
	};
	scope.insert(&hold).await?;
	Ok(hold)
}

/// Retains every File version of the Working Area's current revision for
/// `holder`, as a running Live preview does: its Web server may serve any of
/// them (ADR 0095 amendment).
///
/// Idempotent per `(holder, revision)`: starting again over the same revision
/// returns the existing hold. Refuses with `QUOTA_EXCEEDED` under the same
/// rules as [`retain`].
pub async fn retain_revision(
	scope: &mut dyn RetentionScope,
	area_id: Uuid,
	holder: Holder,
) -> Result<RetentionHold> {
	let area = scope
		.area(area_id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(scope, &area).await?;
	let manifest: Vec<MountedFile> = serde_json::from_value(area.manifest.clone())?;
	let holds = scope.holds(area_id).await?;
	let now = Utc::now();
	if let Some(existing) = holds.iter().find(|hold| {
		hold.holder.same(&holder) && !hold.holder.expired(now) && hold.revision == area.revision
	}) {
		return Ok(existing.clone());
	}
	let limits = scope.limits()?;
	let (working_bytes, retained_limit) = quota(&limits);
	let current = manifest.iter().map(|file| file.size).sum::<u64>();
	let retained = retained_bytes(&holds, &manifest, now);
	if !area_permits_retention(current, retained, working_bytes) {
		return Err(Error::Conflict(
			"QUOTA_EXCEEDED: a Live preview of this revision would exceed the Working Area's byte limit"
				.into(),
		));
	}
	let used = scope.used_bytes().await?;
	let reclaimable = scope.reclaimable_bytes().await?;
	if !permits_retention(used, reclaimable, retained_limit) {
		return Err(Error::Conflict(
			"QUOTA_EXCEEDED: the Tenant's storage cannot keep this revision for a Live preview"
				.into(),
		));
	}
	let hold = RetentionHold {
		tenant: scope.tenant().to_owned(),
		area_id,
		revision: area.revision,
		holder,
		files: manifest.iter().map(RetainedFile::from).collect(),
	};
	scope.insert(&hold).await?;
	Ok(hold)
}

/// Releases every hold of `holder`.
///
/// Callers own the authority that holder stands for: a message's own removal,
/// a Live preview's stop, or the peer path that released a Federation hold.
/// The next reclamation pass then erases the objects no other hold keeps.
pub async fn release(scope: &mut dyn RetentionScope, holder: &Holder) -> Result<u64> {
	scope.release(holder).await
}

/// The File versions the hold covering `file_id` of `area_id` keeps, for a
/// caller that must serve them as one revision (HTML File previews).
///
/// Runs the same current authority as `retain`. `None` when no active hold
/// covers the version, which a preview reports as no longer retained.
pub async fn retained_manifest(
	scope: &mut dyn RetentionScope,
	area_id: Uuid,
	file_id: Uuid,
) -> Result<Option<Vec<RetainedFile>>> {
	let area = scope
		.area(area_id)
		.await?
		.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(scope, &area).await?;
	scope.manifest_of(area_id, file_id).await
}

#[cfg(test)]
mod tests;
