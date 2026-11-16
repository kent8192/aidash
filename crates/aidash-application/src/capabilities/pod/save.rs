//! Register Scratch Space files in the Working Area, remove them, and copy
//! registered files back into the Scratch Space (#745). The Working Area keeps
//! today's storage: each saved file becomes an immutable object, published as a
//! new Area revision.
use super::files::{CHUNK_BYTES, in_pod};
use crate::{
	Error, Result,
	capabilities::files::{files, publish, require_usable},
	ports::capabilities::{
		execution_pod::{FileManifestRequest, FileReadRequest, FileWriteRequest, ManifestFile},
		files::FileScopePort,
		pod_operations::PodContext,
	},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		errors::area_revision_changed,
		files::{ScratchSave, WorkingCopy, WorkingRemove},
		operations::{FileScope, MAX_AREA_FILES, MountedFile, counted_files},
		sessions::Area,
	},
	registry::rules::{digest, validate_path},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// At most this many paths in one call.
const MAX_PATHS: usize = 64;

/// The whole Scratch Space, or a path in it.
fn root(path: &str) -> Result<String> {
	if matches!(path, "" | "." | "/") {
		return Ok(".".into());
	}
	let path = path.trim_end_matches('/');
	validate_path(path)?;
	Ok(path.to_owned())
}

/// Whether `path` is `root` or lies under it.
fn under(path: &str, root: &str) -> bool {
	root == "." || path == root || path.starts_with(&format!("{root}/"))
}

fn roots(paths: &[String]) -> Result<Vec<String>> {
	if paths.is_empty() || paths.len() > MAX_PATHS {
		return Err(Error::Invalid(format!(
			"paths must name between 1 and {MAX_PATHS} files or directories"
		)));
	}
	paths.iter().map(|path| root(path)).collect()
}

/// The Working Area's revision and Run checks every change shares.
async fn admit(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &Area,
	expected_revision: Option<i64>,
) -> Result<()> {
	scope.authorize(area, "file.write").await?;
	if scope.current_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	if expected_revision.is_some_and(|revision| revision != area.revision) {
		return Err(Error::Conflict(area_revision_changed(area.revision)));
	}
	Ok(())
}

/// What a save would change, decided before a byte is copied. A selection that
/// does not fit is refused whole, naming the first path over a limit.
#[derive(Debug)]
pub struct Plan {
	pub kept: Vec<MountedFile>,
	pub saved: Vec<ManifestFile>,
	pub unchanged: Vec<String>,
	pub removed: Vec<String>,
}

pub fn plan(
	existing: Vec<MountedFile>,
	selected: BTreeMap<String, ManifestFile>,
	roots: &[String],
	sync: bool,
	working_bytes: u64,
	retained_bytes: u64,
) -> Result<Plan> {
	let mut kept = vec![];
	let mut unchanged = vec![];
	let mut removed = vec![];
	for file in existing {
		match selected.get(&file.path) {
			Some(_) if file.is_reference_file() => {
				return Err(Error::Conflict(format!(
					"FILE_EXISTS: {} is a Reference file and cannot be replaced",
					file.path
				)));
			}
			Some(found) if found.sha256 == file.digest => {
				unchanged.push(file.path.clone());
				kept.push(file);
			}
			// A changed file is replaced by the saved one.
			Some(_) => removed.push(file.path),
			None if sync
				&& matches!(file.scope, FileScope::Working)
				&& roots.iter().any(|root| under(&file.path, root)) =>
			{
				removed.push(file.path);
			}
			None => kept.push(file),
		}
	}
	let unchanged_paths: BTreeSet<&String> = unchanged.iter().collect();
	let saved: Vec<ManifestFile> = selected
		.into_values()
		.filter(|file| !unchanged_paths.contains(&file.path))
		.collect();
	removed.retain(|path| !saved.iter().any(|file| file.path == *path));
	let mut count = counted_files(&kept).count();
	let mut bytes = counted_files(&kept).map(|file| file.size).sum::<u64>() + retained_bytes;
	for file in &saved {
		count += 1;
		bytes = bytes.saturating_add(file.size);
		if count > MAX_AREA_FILES {
			return Err(Error::Conflict(format!(
				"SAVE_LIMIT: saving {} would hold more than the Working Area's {MAX_AREA_FILES} files; nothing was saved",
				file.path
			)));
		}
		if bytes > working_bytes {
			return Err(Error::Conflict(format!(
				"SAVE_LIMIT: saving {} would exceed the Working Area's {working_bytes} bytes; nothing was saved",
				file.path
			)));
		}
	}
	Ok(Plan {
		kept,
		saved,
		unchanged,
		removed,
	})
}

/// Register Scratch Space files in the Working Area.
pub async fn save(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &mut Area,
	context: &PodContext,
	input: ScratchSave,
) -> Result<Value> {
	let request_digest = digest(&json!(["file_save", run.id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &request_digest).await? {
		return Ok(cached);
	}
	admit(scope, run, area, input.expected_revision).await?;
	let roots = roots(&input.paths)?;
	let mut selected = BTreeMap::new();
	let mut skipped = BTreeSet::new();
	for root in &roots {
		let request = FileManifestRequest {
			scratch_space_id: context.scratch_space_id().to_owned(),
			root: root.clone(),
			ignore_files: input.ignore_files.clone(),
			exclude: input.exclude.clone(),
			include: input.include.clone(),
			max_files: u32::try_from(MAX_AREA_FILES + 1).expect("bounded"),
			missing_ok: input.sync,
		};
		let manifest = in_pod!(context, context.runtime.file_manifest(&request))?;
		if manifest.truncated {
			return Err(Error::Conflict(format!(
				"SAVE_LIMIT: {root} holds more than the Working Area's {MAX_AREA_FILES} files; narrow it with exclude or ignore_files. Nothing was saved"
			)));
		}
		skipped.extend(manifest.skipped_symlinks);
		for file in manifest.files {
			validate_path(&file.path).map_err(|_| {
				Error::Invalid(format!(
					"INVALID_PATH: {:?} cannot be a Working Area path; exclude it. Nothing was saved",
					file.path
				))
			})?;
			selected.insert(file.path.clone(), file);
		}
	}
	let retained = scope.retained_bytes(area).await?;
	let plan = plan(
		files(area)?,
		selected,
		&roots,
		input.sync,
		scope.limits()?.working_bytes,
		retained,
	)?;
	let mut entries = plan.kept;
	for file in &plan.saved {
		entries.push(copy_in(scope, area, context, run, file).await?);
	}
	area.manifest = json!(entries);
	publish(scope, area).await?;
	let result = json!({
		"area_id": area.id,
		"revision": area.revision,
		"saved": plan.saved.iter().map(|file| &file.path).collect::<Vec<_>>(),
		"unchanged": plan.unchanged.len(),
		"removed": plan.removed,
		"skipped_symlinks": skipped,
	});
	scope
		.cache(input.idempotency_key, &request_digest, &result)
		.await?;
	Ok(result)
}

/// Stream one Scratch Space file into a new Working Area object. The bytes must
/// be the ones the manifest named, so a file changed meanwhile is refused.
async fn copy_in(
	scope: &mut dyn FileScopePort,
	area: &Area,
	context: &PodContext,
	run: &RunMetadata,
	file: &ManifestFile,
) -> Result<MountedFile> {
	let changed = || {
		Error::Conflict(format!(
			"FILE_CHANGED: {} changed while it was saved; save it again. Nothing was saved",
			file.path
		))
	};
	scope.begin_pending(area.id, file.size).await?;
	let mut offset = 0;
	while offset < file.size {
		let request = FileReadRequest {
			scratch_space_id: context.scratch_space_id().to_owned(),
			path: file.path.clone(),
			offset: Some(offset),
			limit: u32::try_from((file.size - offset).min(CHUNK_BYTES as u64)).expect("bounded"),
		};
		let chunk = in_pod!(context, context.runtime.file_read(&request))?;
		let bytes = STANDARD
			.decode(&chunk.data)
			.map_err(|_| Error::External("invalid Runner file encoding".into()))?;
		if bytes.is_empty() || chunk.size != file.size {
			return Err(changed());
		}
		offset += bytes.len() as u64;
		scope.write_pending(&bytes).await?;
	}
	let (file_id, saved_digest) =
		scope
			.finish_pending(&file.sha256)
			.await
			.map_err(|error| match error {
				Error::Conflict(_) => changed(),
				other => other,
			})?;
	Ok(MountedFile {
		file_id,
		path: file.path.clone(),
		digest: saved_digest,
		size: file.size,
		media_type: "application/octet-stream".into(),
		scope: FileScope::Working,
		provenance: json!({"kind": "scratch", "run_id": run.id}),
	})
}

/// Remove registered files, or every registered file under a directory.
pub async fn remove(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &mut Area,
	input: WorkingRemove,
) -> Result<Value> {
	let request_digest = digest(&json!(["file_remove", run.id, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &request_digest).await? {
		return Ok(cached);
	}
	admit(scope, run, area, input.expected_revision).await?;
	let roots = roots(&input.paths)?;
	let entries = files(area)?;
	for root in &roots {
		if !entries
			.iter()
			.any(|file| !file.is_reference_file() && under(&file.path, root))
		{
			return Err(Error::NotFound(format!("file unavailable: {root}")));
		}
	}
	let (removed, kept): (Vec<_>, Vec<_>) = entries.into_iter().partition(|file| {
		!file.is_reference_file() && roots.iter().any(|root| under(&file.path, root))
	});
	area.manifest = json!(kept);
	publish(scope, area).await?;
	let result = json!({
		"area_id": area.id,
		"revision": area.revision,
		"removed": removed.iter().map(|file| &file.path).collect::<Vec<_>>(),
	});
	scope
		.cache(input.idempotency_key, &request_digest, &result)
		.await?;
	Ok(result)
}

/// Copy registered files into the Scratch Space, at their paths or under
/// `destination`. Existing Scratch Space files at those paths are replaced.
pub async fn copy(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &Area,
	context: &PodContext,
	input: WorkingCopy,
) -> Result<Value> {
	let request_digest = digest(&json!(["file_copy", run.id, area.revision, input]));
	if let Some(cached) = scope.cached(input.idempotency_key, &request_digest).await? {
		return Ok(cached);
	}
	// Reading the saved files and writing the Scratch Space share the rule of
	// every tool that writes there: file.read, file.write and the active Run.
	scope.authorize(area, "file.read").await?;
	scope.authorize(area, "file.write").await?;
	if scope.current_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let roots = roots(&input.paths)?;
	let destination = input.destination.as_deref().map(root).transpose()?;
	let entries = files(area)?;
	let mut chosen = vec![];
	for root in &roots {
		let matched: Vec<&MountedFile> = entries
			.iter()
			.filter(|file| under(&file.path, root))
			.collect();
		if matched.is_empty() {
			return Err(Error::NotFound(format!("file unavailable: {root}")));
		}
		chosen.extend(matched);
	}
	chosen.sort_by(|a, b| a.path.cmp(&b.path));
	chosen.dedup_by(|a, b| a.file_id == b.file_id);
	let mut copied = vec![];
	for file in chosen {
		require_usable(scope, file).await?;
		let target = match destination.as_deref() {
			None | Some(".") => file.path.clone(),
			Some(directory) => format!("{directory}/{}", file.path),
		};
		validate_path(&target)?;
		let mut object = scope.open(file).await?;
		let mut offset = 0;
		loop {
			let bytes = object.read_range(offset, CHUNK_BYTES as u64).await?;
			let end = offset + bytes.len() as u64;
			if bytes.is_empty() && end < file.size {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			let request = FileWriteRequest {
				scratch_space_id: context.scratch_space_id().to_owned(),
				run_id: run.id.to_string(),
				path: target.clone(),
				data: STANDARD.encode(&bytes),
				offset,
				final_chunk: end >= file.size,
				mode: None,
			};
			in_pod!(context, context.runtime.file_write(&request))?;
			if end >= file.size {
				break;
			}
			offset = end;
		}
		copied.push(json!({"path": target, "size": file.size, "digest": file.digest}));
	}
	let result = json!({"copied": copied});
	scope
		.cache(input.idempotency_key, &request_digest, &result)
		.await?;
	Ok(result)
}

#[cfg(test)]
mod tests;
