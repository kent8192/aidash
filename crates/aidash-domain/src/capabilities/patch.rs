//! Strict add/update/delete grammar frozen against openai/codex
//! b35a7afbe83d72af4607c67732ab4dade38bacb7 (codex-rs/apply-patch/src/parser.rs).
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
	pub idempotency_key: Uuid,
	pub expected_revision: i64,
	/// Every changed path must have a SHA-256 digest, or null to require absence.
	pub preconditions: std::collections::BTreeMap<String, Option<String>>,
	pub patch: String,
}
pub struct Edit {
	pub path: String,
	pub change: Change,
}
pub enum Change {
	Add(String),
	Delete,
	Update(Vec<Chunk>),
}
#[derive(Default)]
pub struct Chunk {
	pub context: Option<String>,
	pub old: Vec<String>,
	pub new: Vec<String>,
	pub eof: bool,
}
fn malformed() -> Error {
	Error::Invalid("INVALID_PATCH_GRAMMAR".into())
}
pub fn parse(text: &str) -> Result<Vec<Edit>> {
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
		crate::registry::rules::validate_path(path)?;
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
pub fn update(text: &str, chunks: Vec<Chunk>, path: &str) -> Result<String> {
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
#[cfg(test)]
mod tests;
