//! Tree rules, lexical containment and the content digest of a plugin directory.
use super::{PluginFile, Rejection};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_FILES: usize = 512;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 240;

/// Rejects a tree that is too large or holds an invalid or duplicate path.
///
/// The fetching layer resolves symlinks, so this layer never sees one.
pub(super) fn check_tree(files: &[PluginFile]) -> Result<(), Rejection> {
	let total = files
		.iter()
		.fold(0usize, |sum, file| sum.saturating_add(file.bytes.len()));
	if files.len() > MAX_FILES || total > MAX_BYTES {
		return Err(Rejection::PackageLimit);
	}
	let mut seen = BTreeSet::new();
	for file in files {
		if !valid_path(&file.path) || !seen.insert(file.path.as_str()) {
			return Err(Rejection::InvalidPath(file.path.clone()));
		}
	}
	Ok(())
}

/// Dot-prefixed segments are allowed here; Skills apply their own stricter rule.
fn valid_path(path: &str) -> bool {
	!path.is_empty()
		&& path.len() <= MAX_PATH_BYTES
		&& !path.contains('\\')
		&& !path.chars().any(char::is_control)
		&& path
			.split('/')
			.all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// Resolves `.` and `..` segments without touching a file system. `None` means the path climbs
/// above the root; an empty result names the root itself, which callers that need a file reject.
pub(super) fn normalize(path: &str) -> Option<String> {
	let mut kept: Vec<&str> = Vec::new();
	for segment in path.split('/') {
		match segment {
			"" | "." => {}
			".." => {
				kept.pop()?;
			}
			other => kept.push(other),
		}
	}
	Some(kept.join("/"))
}

/// `sha256:` over the files sorted by path, each as `path NUL sha256(bytes) LF`.
pub fn tree_digest(files: &[PluginFile]) -> String {
	let mut entries: Vec<(&str, String)> = files
		.iter()
		.map(|file| {
			(
				file.path.as_str(),
				format!("{:x}", Sha256::digest(&file.bytes)),
			)
		})
		.collect();
	entries.sort();
	let mut hasher = Sha256::new();
	for (path, content) in entries {
		hasher.update(path.as_bytes());
		hasher.update([0]);
		hasher.update(content.as_bytes());
		hasher.update(b"\n");
	}
	format!("sha256:{:x}", hasher.finalize())
}
