//! File tools on the Scratch Space go through the Runner's bounded `/v2` file
//! routes, never through shell commands or the server's disk (#745).
use super::{ensure, tool_error};
use crate::{
	Error, Result,
	ports::capabilities::{
		execution_pod::{
			ExecutionPodError, ExecutionPodErrorCode, FileManifestRequest, FilePatchRequest,
			FileReadRequest, FileSearchRequest, FileWriteRequest,
		},
		pod_operations::PodContext,
	},
};
use aidash_domain::capabilities::files::{Representation, ScratchRead, ScratchSearch, SearchMode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

/// The most a Runner file route moves in one call.
pub const CHUNK_BYTES: usize = 1024 * 1024;
/// The most matches one Runner search returns.
pub const SEARCH_RESULTS: usize = 500;

pub(crate) fn unavailable(error: &ExecutionPodError) -> bool {
	matches!(
		error,
		ExecutionPodError::Remote {
			code: ExecutionPodErrorCode::PodUnavailable,
			..
		}
	)
}

/// Make a Runner call; a stopped Pod is started once and the call repeated.
/// A file call changes nothing when the Pod is unavailable, so repeating it is safe.
/// A helper that failed on a running Pod (`file_helper_failed`) may have changed
/// files, so that call is never repeated.
macro_rules! in_pod {
	($context:expr, $call:expr) => {
		match $call.await {
			Err(error) if crate::capabilities::pod::files::unavailable(&error) => {
				crate::capabilities::pod::ensure($context)
					.await
					.map_err(crate::capabilities::pod::tool_error)?;
				$call.await.map_err(crate::capabilities::pod::tool_error)
			}
			other => other.map_err(crate::capabilities::pod::tool_error),
		}
	};
}
pub(crate) use in_pod;

/// Read text from a Scratch Space file. A read that ends inside a UTF-8
/// character stops before it, and `next_offset` continues from there.
pub async fn read(context: &PodContext, input: ScratchRead, read_bytes: usize) -> Result<Value> {
	aidash_domain::registry::rules::validate_path(&input.path)?;
	if matches!(
		input.representation,
		Some(Representation::Metadata | Representation::ModelInput)
	) {
		if input.offset.is_some() || input.max_bytes.is_some() {
			return Err(Error::Invalid(
				"Scratch Space image reads do not accept text ranges".into(),
			));
		}
		let (digest, bytes) = read_snapshot(context, &input.path, 8 * 1024 * 1024).await?;
		if input
			.expected_digest
			.is_some_and(|expected| expected != digest)
		{
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		let media_type = aidash_domain::provider::detect_image(&bytes);
		let selection = if matches!(input.representation, Some(Representation::ModelInput)) {
			if media_type.is_none() {
				return Err(Error::Invalid(
					"Scratch Space file must be a supported image".into(),
				));
			}
			Some(aidash_domain::media::ScratchImageSelection {
				scratch_path: input.path.clone(),
				expected_digest: digest.clone(),
			})
		} else {
			None
		};
		return Ok(json!({
			"metadata":{"scratch_path":input.path,"digest":digest,"size":bytes.len(),"media_type":media_type},
			"selection":selection
		}));
	}

	let limit = input.max_bytes.unwrap_or(read_bytes);
	if limit == 0 || limit > read_bytes {
		return Err(Error::Invalid("INVALID_READ_RANGE".into()));
	}
	let offset = input.offset.unwrap_or(0);
	let request = FileReadRequest {
		scratch_space_id: context.scratch_space_id().to_owned(),
		path: input.path.clone(),
		offset: Some(offset),
		limit: u32::try_from(limit.min(CHUNK_BYTES)).expect("bounded read"),
	};
	let chunk = in_pod!(context, context.runtime.file_read(&request))?;
	let bytes = STANDARD
		.decode(&chunk.data)
		.map_err(|_| Error::External("invalid Runner file encoding".into()))?;
	let text = match std::str::from_utf8(&bytes) {
		Ok(text) => text,
		// A character cut by the end of the range is read next time.
		Err(error) if error.error_len().is_none() => {
			std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid prefix")
		}
		Err(_) => {
			return Err(Error::Invalid(
				"REPRESENTATION_UNAVAILABLE_OR_INVALID_UTF8_OFFSET".into(),
			));
		}
	};
	let end = offset + text.len() as u64;
	if text.is_empty() && offset < chunk.size {
		return Err(Error::Invalid(
			"READ_BUDGET: max_bytes cannot contain the next UTF-8 character".into(),
		));
	}
	let more = end < chunk.size;
	Ok(json!({
		"path": input.path,
		"content": text,
		"encoding": "utf8",
		"size": chunk.size,
		"next_offset": more.then_some(end),
		"truncated": more,
	}))
}

/// Snapshot a single named file. Verify the bytes after the chunked read
/// against the Runner's manifest so edits during a read cannot mix versions.
pub async fn read_snapshot(
	context: &PodContext,
	path: &str,
	max_bytes: u64,
) -> Result<(String, Vec<u8>)> {
	use sha2::{Digest as _, Sha256};
	aidash_domain::registry::rules::validate_path(path)?;
	let manifest = FileManifestRequest {
		scratch_space_id: context.scratch_space_id().to_owned(),
		root: path.to_owned(),
		ignore_files: Vec::new(),
		include: Vec::new(),
		exclude: Vec::new(),
		max_files: 1,
		after: None,
		missing_ok: false,
	};
	let discovered = in_pod!(context, context.runtime.file_manifest(&manifest))?;
	let [file] = discovered.files.as_slice() else {
		return Err(Error::NotFound(format!(
			"Scratch Space file unavailable: {path}"
		)));
	};
	if file.path != path || discovered.truncated {
		return Err(Error::Invalid(format!("Path must name one file: {path}")));
	}
	if file.size > max_bytes {
		return Err(Error::Invalid(
			"Scratch Space file read exceeds its byte limit".into(),
		));
	}
	let mut bytes = Vec::with_capacity(file.size as usize);
	while (bytes.len() as u64) < file.size {
		let offset = bytes.len() as u64;
		let input = FileReadRequest {
			scratch_space_id: context.scratch_space_id().to_owned(),
			path: path.to_owned(),
			offset: Some(offset),
			limit: (file.size - offset).min(CHUNK_BYTES as u64) as u32,
		};
		let chunk = in_pod!(context, context.runtime.file_read(&input))?;
		let block = STANDARD
			.decode(chunk.data)
			.map_err(|_| Error::External("invalid Runner file encoding".into()))?;
		if chunk.size != file.size
			|| block.is_empty()
			|| block.len() as u64 > u64::from(input.limit)
		{
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		bytes.extend(block);
	}
	let digest = format!("{:x}", Sha256::digest(&bytes));
	if digest != file.sha256 {
		return Err(Error::Conflict("FILE_CHANGED".into()));
	}
	Ok((digest, bytes))
}

/// Search Scratch Space files. In `path` mode the query matches paths, listed
/// from the selection `.gitignore` files leave; otherwise it matches lines.
/// `max_matches` and `max_bytes` are the Deployment's search ceilings, as for
/// the Working Area: a larger `limit` is refused, and matches stop before
/// their JSON exceeds `max_bytes`.
pub async fn search(
	context: &PodContext,
	input: ScratchSearch,
	max_matches: usize,
	max_bytes: usize,
) -> Result<Value> {
	let limit = input.limit.unwrap_or(max_matches);
	if !(1..=max_matches.min(SEARCH_RESULTS)).contains(&limit)
		|| input.query.len() > 1024
		|| (input.query.is_empty() && !matches!(input.mode, SearchMode::Path))
	{
		return Err(Error::Invalid("INVALID_SEARCH_BUDGET".into()));
	}
	if let Some(path) = &input.path {
		aidash_domain::registry::rules::validate_path(path)?;
	}
	if matches!(input.mode, SearchMode::Path) {
		let request = FileManifestRequest {
			scratch_space_id: context.scratch_space_id().to_owned(),
			root: input.path.clone().unwrap_or_else(|| ".".into()),
			ignore_files: aidash_domain::capabilities::files::default_ignore_files(),
			exclude: vec![],
			include: vec![],
			max_files: 10_000,
			after: None,
			missing_ok: false,
		};
		let manifest = in_pod!(context, context.runtime.file_manifest(&request))?;
		let glob = input.glob.as_deref().map(glob_matcher).transpose()?;
		let mut matches = manifest
			.files
			.iter()
			.filter(|file| file.path.contains(&input.query))
			.filter(|file| glob.as_ref().is_none_or(|glob| glob.is_match(&file.path)))
			.map(|file| json!({"path": file.path, "size": file.size}));
		let (page, cut) = within_bytes(matches.by_ref().take(limit), max_bytes);
		let truncated = cut || manifest.truncated || matches.next().is_some();
		return Ok(json!({"matches": page, "truncated": truncated}));
	}
	let request = FileSearchRequest {
		scratch_space_id: context.scratch_space_id().to_owned(),
		pattern: input.query,
		path: input.path,
		glob: input.glob,
		regex: matches!(input.mode, SearchMode::Regex),
		max_results: u32::try_from(limit).expect("bounded search"),
	};
	let found = in_pod!(context, context.runtime.file_search(&request))?;
	let (page, cut) = within_bytes(
		found.matches.iter().map(
			|item| json!({"path": item.path, "location": {"line": item.line}, "snippet": item.text}),
		),
		max_bytes,
	);
	Ok(json!({"matches": page, "truncated": cut || found.truncated}))
}

/// The leading matches whose JSON fits `max_bytes`, and whether any was left out.
fn within_bytes(matches: impl Iterator<Item = Value>, max_bytes: usize) -> (Vec<Value>, bool) {
	let mut page = vec![];
	let mut used = 0_usize;
	for item in matches {
		used = used.saturating_add(item.to_string().len() + 1);
		if used > max_bytes {
			return (page, true);
		}
		page.push(item);
	}
	(page, false)
}

/// A shell-style glob over a whole path, as the Runner's search uses.
fn glob_matcher(glob: &str) -> Result<regex::Regex> {
	let mut pattern = String::from("^");
	for character in glob.chars() {
		match character {
			'*' => pattern.push_str(".*"),
			'?' => pattern.push('.'),
			other => pattern.push_str(&regex::escape(&other.to_string())),
		}
	}
	pattern.push('$');
	regex::RegexBuilder::new(&pattern)
		.size_limit(65536)
		.build()
		.map_err(|_| Error::Invalid("INVALID_GLOB".into()))
}

/// Apply a claimed Scratch patch or look up the receipt of an earlier dispatch.
pub async fn patch(
	context: &PodContext,
	run_id: &str,
	patch: String,
	claim_id: uuid::Uuid,
	apply: bool,
) -> Result<Value> {
	// Every path the patch names is checked here as well as by the Runner: a
	// patch that names one outside the Scratch Space is refused whole.
	aidash_domain::capabilities::patch::parse(&patch)?;
	let request = FilePatchRequest {
		scratch_space_id: context.scratch_space_id().to_owned(),
		run_id: run_id.to_owned(),
		claim_id,
		apply,
		patch,
	};
	let result = in_pod!(context, context.runtime.file_patch(&request))?;
	Ok(json!({"changed": result.changed}))
}

/// Write bytes to a Scratch Space path in bounded chunks. The file appears
/// whole, under its final name, only after the last chunk.
pub async fn write(
	context: &PodContext,
	run_id: &str,
	path: &str,
	bytes: &[u8],
	mode: Option<u32>,
) -> Result<u64> {
	aidash_domain::registry::rules::validate_path(path)?;
	let mut offset = 0;
	loop {
		let end = bytes.len().min(offset + CHUNK_BYTES);
		let request = FileWriteRequest {
			scratch_space_id: context.scratch_space_id().to_owned(),
			run_id: run_id.to_owned(),
			path: path.to_owned(),
			data: STANDARD.encode(&bytes[offset..end]),
			offset: offset as u64,
			final_chunk: end == bytes.len(),
			mode,
		};
		let written = in_pod!(context, context.runtime.file_write(&request))?;
		if end == bytes.len() {
			return Ok(written.size);
		}
		offset = end;
	}
}

/// Start the Pod for a tool that needs it at once.
pub async fn started(context: &PodContext) -> Result<()> {
	ensure(context).await.map_err(tool_error)
}

#[cfg(test)]
mod tests;
