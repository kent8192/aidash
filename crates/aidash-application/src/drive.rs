//! Planning rules shared by HTTP uploads and Scratch Space publications.
use aidash_domain::drive::{File, PublishBase};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct PathState {
	pub last_change: i64,
	pub current: Option<File>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Change {
	Put {
		path: String,
		digest: String,
		size: i64,
		media_type: String,
		index: bool,
	},
	Trash {
		path: String,
	},
}

impl Change {
	pub fn path(&self) -> &str {
		match self {
			Self::Put { path, .. } | Self::Trash { path } => path,
		}
	}
}

pub fn conflicts(
	head: i64,
	base: &PublishBase,
	current: &BTreeMap<String, PathState>,
	changes: &[Change],
) -> Vec<String> {
	changes
		.iter()
		.filter_map(|change| {
			let path = change.path();
			let state = current.get(path);
			let valid = base.for_path(path).is_some_and(|base| {
				!matches!(base, aidash_domain::drive::BaseVersion::Version(version) if version > head)
					&& base.matches(
						state.map(|state| state.last_change),
						state.is_some_and(|state| state.current.is_some()),
					)
			});
			(!valid).then(|| path.to_owned())
		})
		.collect()
}

#[derive(Debug, PartialEq, Eq)]
pub struct Passage<'a> {
	pub start_line: usize,
	pub end_line: usize,
	pub text: &'a str,
}

/// Split at UTF-8 character boundaries and report one-based source lines.
pub fn passages(text: &str) -> impl Iterator<Item = Passage<'_>> {
	let mut offsets = text
		.char_indices()
		.map(|(offset, _)| offset)
		.collect::<Vec<_>>();
	offsets.push(text.len());
	let length = offsets.len() - 1;
	let mut start = 0;
	let mut start_line = 1;
	std::iter::from_fn(move || {
		if start == length {
			return None;
		}
		let end = (start + 2_000).min(length);
		let body = &text[offsets[start]..offsets[end]];
		let end_line = start_line
			+ body
				.trim_end_matches('\n')
				.bytes()
				.filter(|byte| *byte == b'\n')
				.count();
		let passage = Passage {
			start_line,
			end_line,
			text: body,
		};
		let next = if end == length { end } else { end - 200 };
		start_line += text[offsets[start]..offsets[next]]
			.bytes()
			.filter(|byte| *byte == b'\n')
			.count();
		start = next;
		Some(passage)
	})
}
