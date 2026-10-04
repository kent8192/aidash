//! Strict add/update/delete grammar frozen against openai/codex
//! b35a7afbe83d72af4607c67732ab4dade38bacb7 (codex-rs/apply-patch/src/parser.rs).
//! Paths address immutable manifest entries, never the server filesystem.
use super::{contracts::*, objects, service, sessions};
use crate::{Error, Result, authorization::access::Access, domain::Run, store::Store};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

struct Edit {
	path: String,
	change: Change,
}
enum Change {
	Add(String),
	Delete,
	Update(Vec<Chunk>),
}
#[derive(Default)]
struct Chunk {
	context: Option<String>,
	old: Vec<String>,
	new: Vec<String>,
	eof: bool,
}
fn malformed() -> Error {
	Error::Invalid("INVALID_PATCH_GRAMMAR".into())
}
fn parse(text: &str) -> Result<Vec<Edit>> {
	let lines: Vec<_> = text.lines().collect();
	if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
		return Err(malformed());
	}
	let mut i = 1;
	let mut edits = vec![];
	let mut seen = BTreeSet::new();
	while i + 1 < lines.len() {
		let header = lines[i];
		i += 1;
		let (path, mode) = if let Some(path) = header.strip_prefix("*** Add File: ") {
			(path, 0)
		} else if let Some(path) = header.strip_prefix("*** Delete File: ") {
			(path, 1)
		} else if let Some(path) = header.strip_prefix("*** Update File: ") {
			(path, 2)
		} else {
			return Err(malformed());
		};
		objects::validate_path(path)?;
		if !seen.insert(path) {
			return Err(Error::Invalid("DUPLICATE_PATCH_PATH".into()));
		}
		let change = match mode {
			0 => {
				let mut added = String::new();
				while i + 1 < lines.len() && !lines[i].starts_with("*** ") {
					added.push_str(lines[i].strip_prefix('+').ok_or_else(malformed)?);
					added.push('\n');
					i += 1;
				}
				Change::Add(added)
			}
			1 => Change::Delete,
			_ => {
				let mut chunks = vec![];
				while i + 1 < lines.len()
					&& !lines[i].starts_with("*** Add File: ")
					&& !lines[i].starts_with("*** Update File: ")
					&& !lines[i].starts_with("*** Delete File: ")
				{
					let mut chunk = Chunk::default();
					if lines[i] == "@@" {
						i += 1;
					} else if let Some(context) = lines[i].strip_prefix("@@ ") {
						chunk.context = Some(context.into());
						i += 1;
					} else if !chunks.is_empty() {
						return Err(malformed());
					}
					let start = i;
					while i + 1 < lines.len()
						&& !lines[i].starts_with("@@")
						&& !lines[i].starts_with("*** ")
					{
						let line = lines[i];
						match line.as_bytes().first() {
							Some(b'+') => chunk.new.push(line[1..].into()),
							Some(b'-') => chunk.old.push(line[1..].into()),
							Some(b' ') => {
								chunk.old.push(line[1..].into());
								chunk.new.push(line[1..].into());
							}
							_ => return Err(malformed()),
						}
						i += 1;
					}
					if i == start {
						return Err(malformed());
					}
					if lines.get(i) == Some(&"*** End of File") {
						chunk.eof = true;
						i += 1;
					}
					chunks.push(chunk);
				}
				if chunks.is_empty() {
					return Err(malformed());
				}
				Change::Update(chunks)
			}
		};
		edits.push(Edit {
			path: path.into(),
			change,
		});
	}
	if edits.is_empty() {
		return Err(malformed());
	}
	Ok(edits)
}
fn update(text: &str, chunks: Vec<Chunk>, path: &str) -> Result<String> {
	let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
	let mut from = 0;
	for chunk in chunks {
		if let Some(context) = chunk.context {
			let found = lines
				.iter()
				.enumerate()
				.skip(from)
				.find(|(_, line)| **line == context)
				.map(|(i, _)| i + 1);
			from = found.ok_or_else(|| Error::Conflict(format!("PATCH_CONTEXT: {path}")))?;
		}
		let candidates = from..=lines.len();
		let at = candidates
			.into_iter()
			.find(|at| {
				let end = at + chunk.old.len();
				end <= lines.len()
					&& (!chunk.eof || end == lines.len())
					&& lines[*at..end] == chunk.old
			})
			.ok_or_else(|| Error::Conflict(format!("PATCH_HUNK: {path}")))?;
		from = at + chunk.new.len();
		lines.splice(at..at + chunk.old.len(), chunk.new);
	}
	Ok(if lines.is_empty() {
		String::new()
	} else {
		format!("{}\n", lines.join("\n"))
	})
}

pub(crate) async fn apply(
	store: &Store,
	access: &mut Access,
	run: &Run,
	area: &mut Area,
	input: Patch,
) -> Result<Value> {
	if input.patch.len() > store.capabilities.0.limits.patch_bytes
		|| input.preconditions.len() > 4096
	{
		return Err(Error::Invalid("PATCH_LIMIT".into()));
	}
	let edits = parse(&input.patch)?;
	let digest = crate::registry::digest(&json!(["apply_patch", run.id, input]));
	if let Some(result) = sessions::cached(access, input.idempotency_key, &digest).await? {
		return Ok(result);
	}
	service::available(area)?;
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED".into()));
	}
	sessions::authorize(access, area, "file.write").await?;
	if sessions::status(access, area).await?.active_run_id != Some(run.id) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	let current = service::files(area)?;
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
				if file.size > store.capabilities.0.working_bytes.min(16 << 20) {
					return Err(Error::Invalid(
						"PATCH_SOURCE_LIMIT: 16 MiB per text file".into(),
					));
				}
				let bytes = store.capabilities.read(access, file).await?;
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
	if total > store.capabilities.0.working_bytes
		|| manifest.len() + staged.iter().filter(|(_, s)| s.is_some()).count() > 4096
	{
		return Err(Error::Conflict("WORKING_QUOTA".into()));
	}
	let id = uuid::Uuid::new_v4();
	for (path, text) in staged {
		if let Some(text) = text {
			let file = store
				.capabilities
				.text_file(
					access,
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
	service::publish(store, access, area).await?;
	let result = json!({"operation_id":id,"status":"completed","generation":area.generation,"revision":area.revision,"changed_paths":paths,"preimages":preimages.iter().map(|f|f.file_id).collect::<Vec<_>>(),"digest":digest});
	sessions::cache(access, input.idempotency_key, &digest, &result).await?;
	Ok(result)
}
