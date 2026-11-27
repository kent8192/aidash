//! Register Scratch Space files in the Working Area, remove them, and copy
//! registered files back into the Scratch Space (#745). The Working Area keeps
//! today's storage: each saved file becomes an immutable object, published as a
//! new Area revision.
use super::files::{CHUNK_BYTES, in_pod};
use crate::{
	Error, Result,
	capabilities::files::{files, publish, publish_with_known_media, require_usable},
	ports::capabilities::{
		execution_pod::{FileManifestRequest, FileReadRequest, FileWriteRequest, ManifestFile},
		files::{FileScopePort, ScratchPublisher, ScratchStager, ScratchUpload, VerifiedFile},
		pod_operations::PodContext,
	},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		errors::{SAVE_MANIFEST_INCOMPLETE, area_revision_changed},
		files::{ScratchSave, WorkingCopy, WorkingRemove},
		operations::{FileScope, MountedFile, counted_files},
		sessions::Area,
	},
	registry::rules::{digest, validate_path},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

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
	let mut bytes = counted_files(&kept).map(|file| file.size).sum::<u64>() + retained_bytes;
	for file in &saved {
		bytes = bytes.saturating_add(file.size);
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

pub(crate) struct Save {
	input: ScratchSave,
	roots: Vec<String>,
	request_digest: String,
	working_bytes: u64,
	retained_bytes: u64,
}

pub(crate) struct Downloaded<U> {
	plan: Plan,
	uploads: Vec<(U, &'static str)>,
	skipped: BTreeSet<String>,
	input: ScratchSave,
	request_digest: String,
	working_bytes: u64,
}

impl Save {
	pub(crate) async fn prepare(
		scope: &mut dyn FileScopePort,
		run: &RunMetadata,
		area: &Area,
		input: ScratchSave,
	) -> Result<super::super::files::Prepared<Self>> {
		use super::super::files::Prepared;
		let request_digest = digest(&json!(["file_save", run.id, input]));
		if let Some(cached) = scope.cached(input.idempotency_key, &request_digest).await? {
			return Ok(Prepared::Replay(cached));
		}
		admit(scope, run, area, input.expected_revision).await?;
		Ok(Prepared::Call(Self {
			roots: roots(&input.paths)?,
			working_bytes: scope.limits()?.working_bytes,
			retained_bytes: scope.retained_bytes(area).await?,
			input,
			request_digest,
		}))
	}

	pub(crate) async fn download<S: ScratchStager>(
		self,
		stage: &mut S,
		area: &Area,
		context: &PodContext,
	) -> Result<Downloaded<<S::Upload as ScratchUpload>::Sealed>> {
		let Self {
			input,
			roots,
			request_digest,
			working_bytes,
			retained_bytes,
		} = self;
		let mut selected = BTreeMap::new();
		let mut skipped = BTreeSet::new();
		for root in &roots {
			let request = FileManifestRequest {
				scratch_space_id: context.scratch_space_id().to_owned(),
				root: root.clone(),
				ignore_files: input.ignore_files.clone(),
				exclude: input.exclude.clone(),
				include: input.include.clone(),
				max_files: None,
				missing_ok: input.sync,
			};
			let manifest = in_pod!(context, context.runtime.file_manifest(&request))?;
			if manifest.truncated {
				return Err(Error::Conflict(format!(
					"{SAVE_MANIFEST_INCOMPLETE}: incomplete manifest for {root}. Nothing was saved."
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
		let plan = plan(
			files(area)?,
			selected,
			&roots,
			input.sync,
			working_bytes,
			retained_bytes,
		)?;
		let mut uploads = Vec::with_capacity(plan.saved.len());
		for file in &plan.saved {
			let mut upload = stage.stage(area.id, file.size).await?;
			let media = download_file(&mut upload, context, file).await?;
			let upload = upload
				.seal(&file.sha256)
				.await
				.map_err(|error| match error {
					Error::Conflict(message) if message == "OBJECT_INTEGRITY" => file_changed(file),
					other => other,
				})?;
			uploads.push((upload, media));
		}
		Ok(Downloaded {
			plan,
			uploads,
			skipped,
			input,
			request_digest,
			working_bytes,
		})
	}
}

impl<U: Send> Downloaded<U> {
	pub(crate) async fn publish(
		self,
		scope: &mut dyn ScratchPublisher<Upload = U>,
		run: &RunMetadata,
		area: &mut Area,
		revision: i64,
	) -> Result<Value> {
		admit(scope, run, area, None).await?;
		if let Some(cached) = scope
			.cached(self.input.idempotency_key, &self.request_digest)
			.await?
		{
			return Ok(cached);
		}
		if area.revision != revision {
			return Err(Error::Conflict(area_revision_changed(area.revision)));
		}
		let Plan {
			kept,
			saved,
			unchanged,
			removed,
		} = self.plan;
		// Retained versions can change without changing the Area revision.
		let bytes = counted_files(&kept)
			.map(|file| file.size)
			.chain(saved.iter().map(|file| file.size))
			.fold(scope.retained_bytes(area).await?, u64::saturating_add);
		if !saved.is_empty() && bytes > self.working_bytes {
			return Err(Error::Conflict(
				"SAVE_LIMIT: Working Area byte limit changed while saving".into(),
			));
		}
		let mut entries = kept;
		let mut known_media = BTreeSet::new();
		for (file, (upload, media)) in saved.iter().zip(self.uploads) {
			let (file_id, saved_digest) = scope.publish_scratch_upload(upload).await?;
			known_media.insert(file_id);
			entries.push(MountedFile {
				file_id,
				path: file.path.clone(),
				digest: saved_digest,
				size: file.size,
				media_type: media.into(),
				scope: FileScope::Working,
				provenance: json!({"kind":"scratch","run_id":run.id}),
			});
		}
		area.manifest = json!(entries);
		publish_with_known_media(scope, area, &known_media).await?;
		let result = json!({
			"area_id":area.id, "revision":area.revision,
			"saved":saved.iter().map(|file| &file.path).collect::<Vec<_>>(),
			"unchanged":unchanged.len(), "removed":removed, "skipped_symlinks":self.skipped,
		});
		scope
			.cache(self.input.idempotency_key, &self.request_digest, &result)
			.await?;
		Ok(result)
	}
}

fn file_changed(file: &ManifestFile) -> Error {
	Error::Conflict(format!(
		"FILE_CHANGED: {} changed while it was saved; save it again. Nothing was saved",
		file.path
	))
}

/// Read the manifest's bytes into the staged upload, checking its size.
async fn download_file(
	upload: &mut impl ScratchUpload,
	context: &PodContext,
	file: &ManifestFile,
) -> Result<&'static str> {
	let mut offset = 0;
	// Enough leading bytes to recognize an image, as command output publication did.
	let mut prefix = Vec::with_capacity(12);
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
		if bytes.is_empty() || chunk.size != file.size || bytes.len() as u64 > file.size - offset {
			return Err(file_changed(file));
		}
		let needed = 12_usize.saturating_sub(prefix.len());
		prefix.extend_from_slice(&bytes[..bytes.len().min(needed)]);
		offset += bytes.len() as u64;
		upload.write_block(&bytes).await?;
	}
	Ok(media_type(&prefix))
}

/// The media type a saved file records: an image the models accept is
/// recognized by its signature, so `file_read` can give it to a model.
fn media_type(prefix: &[u8]) -> &'static str {
	aidash_domain::provider::detect_image(prefix).unwrap_or("application/octet-stream")
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
pub(crate) struct Copy {
	key: Uuid,
	digest: String,
	files: Vec<CopySource>,
}

struct CopySource {
	file: MountedFile,
	target: String,
	object: Box<dyn VerifiedFile>,
}

pub(crate) async fn admit_copy(
	scope: &mut dyn FileScopePort,
	run: &RunMetadata,
	area: &Area,
) -> Result<()> {
	scope.authorize(area, "file.read").await?;
	scope.authorize(area, "file.write").await?;
	if scope.current_run(area).await? != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	Ok(())
}

impl Copy {
	pub(crate) async fn prepare(
		scope: &mut dyn FileScopePort,
		run: &RunMetadata,
		area: &Area,
		input: WorkingCopy,
	) -> Result<super::super::files::Prepared<Self>> {
		use super::super::files::Prepared;
		let digest = digest(&json!(["file_copy", run.id, area.revision, input]));
		if let Some(cached) = scope.cached(input.idempotency_key, &digest).await? {
			return Ok(Prepared::Replay(cached));
		}
		admit_copy(scope, run, area).await?;
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
		let mut files = Vec::with_capacity(chosen.len());
		for file in chosen {
			require_usable(scope, file).await?;
			let target = match destination.as_deref() {
				None | Some(".") => file.path.clone(),
				Some(directory) => format!("{directory}/{}", file.path),
			};
			validate_path(&target)?;
			let object = scope.open_ranges(file).await?;
			files.push(CopySource {
				file: file.clone(),
				target,
				object,
			});
		}
		Ok(Prepared::Call(Self {
			key: input.idempotency_key,
			digest,
			files,
		}))
	}

	pub(crate) async fn execute(
		&mut self,
		run: &RunMetadata,
		context: &PodContext,
	) -> Result<Value> {
		let mut copied = Vec::with_capacity(self.files.len());
		for CopySource {
			file,
			target,
			object,
		} in &mut self.files
		{
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
			copied.push(json!({"path":target,"size":file.size,"digest":file.digest}));
		}
		Ok(json!({"copied":copied}))
	}

	pub(crate) async fn settle(
		self,
		scope: &mut dyn FileScopePort,
		run: &RunMetadata,
		area: &Area,
		result: Value,
	) -> Result<Value> {
		admit_copy(scope, run, area).await?;
		for CopySource { file, .. } in self.files {
			require_usable(scope, &file).await?;
		}
		if let Some(cached) = scope.cached(self.key, &self.digest).await? {
			return Ok(cached);
		}
		scope.cache(self.key, &self.digest, &result).await?;
		Ok(result)
	}
}

#[cfg(test)]
mod tests;
