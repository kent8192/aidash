//! Document snapshots and the Evidence fragments delivered from them. A
//! snapshot is immutable: continuations and fragments are bound to its text
//! digest, so a later retrieval of the same address never changes them.
use super::Operation;
use crate::{Error, Result};
use base64::Engine;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Text retained per snapshot, in UTF-8 bytes.
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
pub const MAX_LINES: usize = 20_000;
pub const MAX_LINE_BYTES: usize = 16 * 1024;
/// The largest original a snapshot retains (ADR 0053).
pub const MAX_ORIGINAL_BYTES: usize = 16 * 1024 * 1024;
/// The largest page count a snapshot records, the range its page numbers are
/// stored in.
pub const MAX_PAGES: u32 = i32::MAX as u32;
/// The most pages a snapshot lists as carrying no text. Page reading reads
/// no more pages than this, and the list, like the rest of a result, must fit
/// within [`super::MAX_RESULT_BYTES`].
pub const MAX_PAGES_WITHOUT_TEXT: usize = 500;
/// Bytes of snapshot text one fetch delivers, unless the call asks for fewer.
pub const DEFAULT_DELIVERY_BYTES: usize = 8 * 1024;
pub const MAX_DELIVERY_BYTES: usize = 16 * 1024;
pub const MAX_FIND_MATCHES: usize = 10;
pub const MAX_FIND_QUERY_CHARS: usize = 200;
/// Identifies the folding rule a `find` result was computed with.
pub const CASE_FOLDING: &str = "aidash-fold/1 (lowercase; ß and ẞ as ss)";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Line {
	pub text: String,
	/// 1-based page of a paginated source such as a PDF.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
	Complete,
	/// The page had more text than a snapshot retains.
	TextLimit,
}

impl Completeness {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Complete => "complete",
			Self::TextLimit => "text_limit",
		}
	}
}

/// Readable text a page provider extracted from one retrieval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedPage {
	/// The address that answered, after any permitted redirect.
	pub url: String,
	pub title: Option<String>,
	pub media_type: String,
	pub fetched_at: DateTime<Utc>,
	/// `sha256:` digest of the retrieved bytes.
	pub raw_digest: String,
	pub completeness: Completeness,
	pub lines: Vec<Line>,
	/// The page count of a paginated source such as a PDF.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page_count: Option<u32>,
	/// 1-based pages that carry no text, in page order.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pages_without_text: Vec<u32>,
	/// The retrieved document, present only when some page carries no text, so
	/// that the Run's model can view that page (ADR 0053).
	#[serde(skip)]
	pub original: Option<Vec<u8>>,
}

impl ExtractedPage {
	/// Provider output is untrusted; the host enforces the snapshot bounds.
	/// A paginated source may have no text lines when its pages carry none.
	pub fn validate(&self) -> Result<()> {
		let bytes = self.lines.iter().map(|line| line.text.len()).sum::<usize>();
		let within = |page: u32| page > 0 && self.page_count.is_none_or(|count| page <= count);
		if (self.lines.is_empty() && self.pages_without_text.is_empty())
			|| self.page_count == Some(0)
			|| self.page_count.is_some_and(|count| count > MAX_PAGES)
			|| self.lines.len() > MAX_LINES
			|| self
				.lines
				.iter()
				.any(|line| line.page.is_some_and(|page| !within(page)))
			|| (!self.pages_without_text.is_empty() && self.page_count.is_none())
			|| self.pages_without_text.len() > MAX_PAGES_WITHOUT_TEXT
			|| !self.pages_without_text.iter().all(|page| within(*page))
			|| !self
				.pages_without_text
				.windows(2)
				.all(|pair| pair[0] < pair[1])
			// A page either carries text or is listed as carrying none. The
			// list is sorted by the check above.
			|| self.lines.iter().any(|line| {
				line.page
					.is_some_and(|page| self.pages_without_text.binary_search(&page).is_ok())
			})
			// A complete paginated source accounts for every page.
			|| (self.completeness == Completeness::Complete
				&& self.page_count.is_some_and(|count| !self.covers(count)))
			// The original travels exactly when some page carries no text; the
			// host, not the provider, decides whether it is retained.
			|| match &self.original {
				Some(original) => {
					self.pages_without_text.is_empty()
						|| original.is_empty()
						|| original.len() > MAX_ORIGINAL_BYTES
				}
				None => !self.pages_without_text.is_empty(),
			} || bytes > MAX_SNAPSHOT_BYTES
			|| self.lines.iter().any(|line| {
				line.text.len() > MAX_LINE_BYTES
					|| line.page == Some(0)
					|| line.text.chars().any(|c| c.is_control() && c != '\t')
			}) || self
			.title
			.as_ref()
			.is_some_and(|title| title.len() > 512 || title.chars().any(char::is_control))
			|| super::search::candidate_host(&self.url).is_none()
			|| !self.raw_digest.starts_with("sha256:")
		{
			return Err(Error::Invalid(
				"extracted page violates snapshot bounds".into(),
			));
		}
		Ok(())
	}

	/// Whether every page through `count` carries text or is listed as
	/// carrying none. [`Self::validate`] keeps every page it names within
	/// `count`, so counting the distinct pages suffices.
	fn covers(&self, count: u32) -> bool {
		let mut pages = self
			.lines
			.iter()
			.filter_map(|line| line.page)
			.collect::<std::collections::BTreeSet<_>>();
		pages.extend(&self.pages_without_text);
		usize::try_from(count).is_ok_and(|count| pages.len() == count)
	}
}

/// `sha256:` digest of a snapshot's lines; it identifies the snapshot revision.
pub fn text_digest(lines: &[Line]) -> String {
	let encoded = serde_json::to_vec(lines).expect("serializable lines");
	format!("sha256:{:x}", Sha256::digest(encoded))
}

/// A position in a snapshot: line index and byte offset within that line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
	pub line: usize,
	pub offset: usize,
}

/// One delivered passage, before it is retained as an Evidence fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
	pub text: String,
	/// 1-based, inclusive.
	pub line_start: usize,
	pub line_end: usize,
	pub pages: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
	pub passage: Passage,
	pub next: Option<Position>,
}

/// Deliver at most `max_bytes` of text from `from`. Lines are joined with a
/// newline; a line is split only at a character boundary. A positive budget
/// smaller than the next character still delivers that one character, so
/// every valid budget makes progress. When the separator after a completed
/// line does not fit, the continuation stays at the end of that line and the
/// next delivery begins with the separator, so no newline is ever lost.
pub fn deliver(lines: &[Line], from: Position, max_bytes: usize) -> Result<Delivery> {
	let Position {
		mut line,
		mut offset,
	} = from;
	if line >= lines.len()
		|| offset > lines[line].text.len()
		|| !lines[line].text.is_char_boundary(offset)
	{
		return Err(Error::Invalid("invalid continuation position".into()));
	}
	if max_bytes == 0 {
		return Err(Error::Invalid("max_bytes must be positive".into()));
	}
	let mut text = String::new();
	let mut pages = Vec::new();
	// A continuation at the end of a non-final line resumes with its separator.
	if offset == lines[line].text.len() && line + 1 < lines.len() {
		text.push('\n');
		line += 1;
		offset = 0;
	}
	let line_start = line + 1;
	let mut line_end = line_start;
	while line < lines.len() && text.len() < max_bytes {
		let value = &lines[line].text;
		let room = max_bytes - text.len();
		let mut end = (offset + room).min(value.len());
		while !value.is_char_boundary(end) {
			end -= 1;
		}
		if end == offset && offset < value.len() {
			if !text.is_empty() {
				break;
			}
			end = offset + value[offset..].chars().next().map_or(0, char::len_utf8);
		}
		text.push_str(&value[offset..end]);
		line_end = line + 1;
		if let Some(page) = lines[line].page
			&& !pages.contains(&page)
		{
			pages.push(page);
		}
		offset = end;
		if offset < value.len() {
			break;
		}
		if line + 1 == lines.len() {
			line += 1;
			offset = 0;
			break;
		}
		// The completed line's separator is delivered only with room for it.
		if text.len() + 1 > max_bytes {
			break;
		}
		text.push('\n');
		line += 1;
		offset = 0;
	}
	Ok(Delivery {
		passage: Passage {
			text,
			line_start,
			line_end,
			pages,
		},
		next: (line < lines.len()).then_some(Position { line, offset }),
	})
}

/// Case-insensitive comparison form. It lowercases and expands the sharp s so
/// that `STRASSE` finds `Straße`; it does not normalize other compatibility forms.
pub fn fold(text: &str) -> String {
	text.chars().flat_map(char::to_lowercase).fold(
		String::with_capacity(text.len()),
		|mut folded, c| {
			if c == 'ß' {
				folded.push_str("ss");
			} else {
				folded.push(c);
			}
			folded
		},
	)
}

/// Indexes of lines from `start` containing `query`, and the line to resume
/// from when more than `max` lines match.
pub fn find(
	lines: &[Line],
	query: &str,
	case_sensitive: bool,
	start: usize,
	max: usize,
) -> (Vec<usize>, Option<usize>) {
	let needle = if case_sensitive {
		query.to_owned()
	} else {
		fold(query)
	};
	let mut matches = Vec::new();
	for (index, line) in lines.iter().enumerate().skip(start) {
		let found = if case_sensitive {
			line.text.contains(&needle)
		} else {
			fold(&line.text).contains(&needle)
		};
		if !found {
			continue;
		}
		if matches.len() == max {
			return (matches, Some(index));
		}
		matches.push(index);
	}
	(matches, None)
}

/// A continuation token. It is bound to the Run's snapshot, its text digest,
/// the operation and (for `find`) the exact query, so it cannot address another
/// document or request. It grants nothing: the snapshot is checked again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
	pub document: Uuid,
	pub digest: String,
	pub operation: Operation,
	pub position: Position,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub binding: Option<String>,
}

impl Cursor {
	pub fn encode(&self) -> String {
		let json = serde_json::to_vec(self).expect("serializable cursor");
		format!(
			"c1.{}",
			base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
		)
	}
	pub fn decode(token: &str) -> Result<Self> {
		let invalid = || Error::Invalid("invalid continuation".into());
		let encoded = token.strip_prefix("c1.").ok_or_else(invalid)?;
		if encoded.len() > 1024 {
			return Err(invalid());
		}
		let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
			.decode(encoded)
			.map_err(|_| invalid())?;
		serde_json::from_slice(&bytes).map_err(|_| invalid())
	}
	/// Accept only a continuation issued for this exact snapshot and request.
	pub fn check(
		&self,
		document: Uuid,
		digest: &str,
		operation: Operation,
		binding: Option<&str>,
	) -> Result<Position> {
		if self.document != document
			|| self.digest != digest
			|| self.operation != operation
			|| self.binding.as_deref() != binding
		{
			return Err(Error::Invalid("invalid continuation".into()));
		}
		Ok(self.position)
	}
}

/// The binding of a `find` continuation to its exact query.
pub fn find_binding(query: &str, case_sensitive: bool) -> String {
	let encoded = serde_json::to_vec(&(query, case_sensitive)).expect("serializable query");
	format!("sha256:{:x}", Sha256::digest(encoded))
}

#[cfg(test)]
mod tests;
