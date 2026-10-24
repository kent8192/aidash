//! All hunks and preimages are checked before allocation; one manifest publication exposes the batch.
use crate::{Error, Result, capabilities::files, ports::capabilities::patch::PatchScope};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::{FileScope, MountedFile as FileEntry, available},
		patch::{Change, Patch, parse, update},
		sessions::Area,
	},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub async fn apply(
	scope: &mut dyn PatchScope,
	run: &RunMetadata,
	area: &mut Area,
	input: Patch,
) -> Result<Value> {
	if input.patch.len() > scope.patch_limits()?.0 || input.preconditions.len() > 4096 {
		return Err(Error::Invalid("PATCH_LIMIT".into()));
	}
	let edits = parse(&input.patch)?;
	let digest = aidash_domain::registry::rules::digest(&json!(["apply_patch", run.id, input]));
	if let Some(result) = scope.cached(input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	available(&area.state)?;
	if !scope.patch_limits()?.1 {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	scope.authorize(area, "file.write").await?;
	if scope.current_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let current = serde_json::from_value::<Vec<FileEntry>>(area.manifest.clone())?;
	let mut manifest: BTreeMap<_, _> = current.into_iter().map(|f| (f.path.clone(), f)).collect();
	let paths: BTreeSet<_> = edits.iter().map(|e| e.path.clone()).collect();
	if paths != input.preconditions.keys().cloned().collect() {
		return Err(Error::Invalid("PATCH_PRECONDITIONS_REQUIRED".into()));
	}
	let actual: BTreeMap<_, _> = paths
		.iter()
		.map(|p| (p.clone(), manifest.get(p).map(|f| f.digest.clone())))
		.collect();
	if area.revision != input.expected_revision || actual != input.preconditions {
		return Err(Error::Conflict(format!(
			"PATCH_CONFLICT: {}",
			json!({"revision":area.revision,"actual":actual})
		)));
	}
	// Validate ALL hunks before allocating bytes. Existing immutable objects are
	// the restricted preimages; their area ownership and quota are unchanged.
	let mut staged = vec![];
	let mut preimages = vec![];
	for edit in edits {
		let old = manifest.get(&edit.path);
		if old.is_some_and(|f| !matches!(f.scope, FileScope::Working)) {
			return Err(Error::Forbidden);
		}
		if let Some(file) = old {
			preimages.push(file.clone());
		}
		let next = match edit.change {
			Change::Add(text) => {
				if old.is_some() {
					return Err(Error::Conflict(format!("FILE_EXISTS: {}", edit.path)));
				}
				Some(text)
			}
			Change::Delete => {
				if old.is_none() {
					return Err(Error::Conflict(format!("FILE_ABSENT: {}", edit.path)));
				}
				None
			}
			Change::Update(chunks) => {
				let file =
					old.ok_or_else(|| Error::Conflict(format!("FILE_ABSENT: {}", edit.path)))?;
				if file.size > scope.limits()?.working_bytes.min(16 << 20) {
					return Err(Error::Invalid(
						"PATCH_SOURCE_LIMIT: 16 MiB per text file".into(),
					));
				}
				let bytes = scope.read_file(file).await?;
				let text = std::str::from_utf8(&bytes)
					.map_err(|_| Error::Invalid("PATCH_REQUIRES_UTF8".into()))?;
				Some(update(text, chunks, &edit.path)?)
			}
		};
		manifest.remove(&edit.path);
		staged.push((edit.path, next));
	}
	let total = manifest.values().map(|f| f.size).sum::<u64>()
		+ staged
			.iter()
			.map(|(_, s)| s.as_ref().map_or(0, |s| s.len() as u64))
			.sum::<u64>();
	if total > scope.limits()?.working_bytes
		|| manifest.len() + staged.iter().filter(|(_, s)| s.is_some()).count() > 4096
	{
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let id = uuid::Uuid::new_v4();
	for (path, text) in staged {
		if let Some(text) = text {
			let file = scope
				.text_file(
					area.id,
					path.clone(),
					&text,
					FileScope::Working,
					json!({"kind":"patch","operation_id":id}),
				)
				.await?;
			manifest.insert(path, file);
		}
	}
	// Each object and its ownership intent is fsynced before the single PG
	// publication. A crash before commit exposes the old manifest; after commit
	// it exposes all new bytes. No live path is renamed or overwritten.
	area.manifest = json!(manifest.values().collect::<Vec<_>>());
	files::publish(scope, area).await?;
	let result = json!({"operation_id":id,"status":"completed","generation":area.generation,"revision":area.revision,"changed_paths":paths,"preimages":preimages.iter().map(|f|f.file_id).collect::<Vec<_>>(),"digest":digest});
	scope.cache(input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
