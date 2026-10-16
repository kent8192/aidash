//! Run-scoped Research evidence as the host retains it. Records describe what
//! was examined at retrieval time, even if the live page later changes.
use super::{
	Operation,
	citations::EvidenceRef,
	document::{Completeness, Line, Passage},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// A candidate Web source discovered or supplied for a Run. It does not imply
/// that its contents have been examined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
	pub id: Uuid,
	pub run_id: Uuid,
	pub url: String,
	pub title: String,
}

/// The fixed contents of one retrieval of a Web source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSnapshot {
	pub id: Uuid,
	pub run_id: Uuid,
	pub source_id: Uuid,
	pub requested_url: String,
	pub url: String,
	pub title: Option<String>,
	pub media_type: String,
	pub fetched_at: DateTime<Utc>,
	pub raw_digest: String,
	pub text_digest: String,
	pub completeness: Completeness,
	/// Empty once the snapshot is purged.
	pub lines: Vec<Line>,
	/// The page count of a paginated source such as a PDF.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub page_count: Option<u32>,
	/// 1-based pages that carry no text; their images come from the original.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub pages_without_text: Vec<u32>,
	/// The original document, recorded when some page carries no text.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub original: Option<Original>,
	/// Set when the contents were removed; identity and digests remain.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub purged: Option<Purge>,
}

/// The original of a snapshot with pages that carry no text (ADR 0053). Its
/// bytes are kept apart from the snapshot; `retained` is false when the Run's
/// limit refused them or they were purged with the contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Original {
	/// `sha256:` digest of the original bytes.
	pub digest: String,
	pub bytes: u64,
	pub retained: bool,
}

impl DocumentSnapshot {
	/// The contents can no longer be read or searched; a Purged snapshot is
	/// never mistaken for a page that was never read.
	pub fn readable(&self) -> Result<(), super::Failure> {
		match self.purged {
			None => Ok(()),
			Some(Purge {
				reason: PurgeReason::Revocation,
				..
			}) => Err(super::Failure::EvidenceRevoked),
			Some(_) => Err(super::Failure::SnapshotPurged),
		}
	}
}

/// Why a Document snapshot's contents were removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PurgeReason {
	/// The Retention Rules no longer keep the contents.
	Retention,
	/// An Evidence Revocation removed the contents.
	Revocation,
}

impl PurgeReason {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Retention => "retention",
			Self::Revocation => "revocation",
		}
	}
	pub fn parse(reason: &str) -> Option<Self> {
		Some(match reason {
			"retention" => Self::Retention,
			"revocation" => Self::Revocation,
			_ => return None,
		})
	}
}

/// When and why a Purged snapshot lost its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Purge {
	pub at: DateTime<Utc>,
	pub reason: PurgeReason,
}

/// A passage of a Document snapshot delivered to the Agent and eligible to
/// support a Source citation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceFragment {
	pub reference: EvidenceRef,
	pub run_id: Uuid,
	pub document_id: Uuid,
	pub source_id: Uuid,
	pub text_digest: String,
	pub text: String,
	pub line_start: usize,
	pub line_end: usize,
	pub pages: Vec<u32>,
}

impl EvidenceFragment {
	pub fn new(reference: EvidenceRef, document: &DocumentSnapshot, passage: &Passage) -> Self {
		Self {
			reference,
			run_id: document.run_id,
			document_id: document.id,
			source_id: document.source_id,
			text_digest: document.text_digest.clone(),
			text: passage.text.clone(),
			line_start: passage.line_start,
			line_end: passage.line_end,
			pages: passage.pages.clone(),
		}
	}
	/// The fragment as delivered to the model, with its snapshot provenance.
	pub fn delivered(&self, document: &DocumentSnapshot) -> Value {
		json!({
			"evidence_ref": self.reference.to_string(),
			"source_id": self.source_id,
			"document_id": self.document_id,
			"url": document.url,
			"title": document.title,
			"fetched_at": document.fetched_at,
			"text_digest": self.text_digest,
			"completeness": document.completeness.as_str(),
			"text": self.text,
			"line_start": self.line_start,
			"line_end": self.line_end,
			"pages": self.pages,
		})
	}
}

/// Whole snapshot lines shown on each side of a cited Evidence fragment.
pub const CONTEXT_LINES: usize = 20;
/// UTF-8 bytes of snapshot text shown on each side of a cited Evidence fragment.
pub const CONTEXT_BYTES: usize = 4 * 1024;

/// Document snapshot lines around a cited Evidence fragment. They were not
/// delivered to the Agent with the fragment; a reader sees them only to judge
/// the passage in its place. Each side holds whole lines, nearest first, up to
/// [`CONTEXT_LINES`] lines and [`CONTEXT_BYTES`] bytes, kept in document order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotContext {
	pub before: Vec<Line>,
	pub after: Vec<Line>,
}

impl SnapshotContext {
	/// `line_start` and `line_end` are the fragment's 1-based, inclusive lines.
	pub fn around(lines: &[Line], line_start: usize, line_end: usize) -> Self {
		let before_end = line_start.saturating_sub(1).min(lines.len());
		let mut before = bounded(lines[..before_end].iter().rev());
		before.reverse();
		let after_start = line_end.min(lines.len());
		Self {
			before,
			after: bounded(lines[after_start..].iter()),
		}
	}
}

/// Whole lines from `nearest` until a side's line or byte bound; a line that
/// would cross the byte bound ends the side rather than being cut.
fn bounded<'a>(nearest: impl Iterator<Item = &'a Line>) -> Vec<Line> {
	let mut bytes = 0;
	nearest
		.take(CONTEXT_LINES)
		.take_while(|line| {
			bytes += line.text.len();
			bytes <= CONTEXT_BYTES
		})
		.cloned()
		.collect()
}

/// One Source citation as a reader opens it: the Evidence fragment delivered
/// to the Agent and the Document snapshot it was taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedEvidence {
	pub fragment: EvidenceFragment,
	/// The address the Run asked for; `url` is the one that answered.
	pub requested_url: String,
	pub url: String,
	pub title: Option<String>,
	pub media_type: String,
	pub fetched_at: DateTime<Utc>,
	pub completeness: Completeness,
	/// `None` once the snapshot's text is no longer retained; the fragment
	/// itself stays readable.
	pub context: Option<SnapshotContext>,
}

impl CitedEvidence {
	pub fn new(fragment: EvidenceFragment, document: &DocumentSnapshot) -> Self {
		let context = (!document.lines.is_empty()).then(|| {
			SnapshotContext::around(&document.lines, fragment.line_start, fragment.line_end)
		});
		Self {
			fragment,
			requested_url: document.requested_url.clone(),
			url: document.url.clone(),
			title: document.title.clone(),
			media_type: document.media_type.clone(),
			fetched_at: document.fetched_at,
			completeness: document.completeness,
			context,
		}
	}
}

/// How one research attempt ended. Every attempt the provider was called for
/// is attributed, whether it answered, failed definitively or its outcome is
/// uncertain; only an attempt stopped before the provider call is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
	/// The provider was called and answered, with a result or a definitive
	/// failure such as an HTTP error or an invalid response.
	Completed,
	/// Stopped after the reservation without calling the provider.
	Failed,
	/// The provider may have performed the request.
	Unknown,
}

impl AttemptOutcome {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Completed => "completed",
			Self::Failed => "failed",
			Self::Unknown => "unknown",
		}
	}
	pub fn charged(self) -> bool {
		!matches!(self, Self::Failed)
	}
}

/// A dispatched or settled provider attempt of one invocation key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptState {
	/// Reserved and possibly sent; the result was never recorded.
	Dispatched,
	Settled {
		outcome: AttemptOutcome,
		result: Value,
	},
}

/// Counters a Run's research limits are checked against before dispatch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunUsage {
	pub search_attempts: u32,
	pub fetch_attempts: u32,
	pub sources: u32,
	pub snapshots: u32,
	pub snapshot_bytes: u64,
}

impl RunUsage {
	pub fn attempts(&self, operation: Operation) -> u32 {
		match operation {
			Operation::Search => self.search_attempts,
			Operation::Fetch => self.fetch_attempts,
			Operation::Find => 0,
		}
	}
}

/// Per-Run research limits; reaching one blocks dispatch and disclosure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
	pub search_attempts: u32,
	pub fetch_attempts: u32,
	pub sources: u32,
	pub snapshots: u32,
	pub snapshot_bytes: u64,
	/// Bytes of originals one Run retains (ADR 0053).
	#[serde(default = "default_original_bytes")]
	pub original_bytes: u64,
}

fn default_original_bytes() -> u64 {
	64 * 1024 * 1024
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			search_attempts: 20,
			fetch_attempts: 50,
			sources: 100,
			snapshots: 50,
			snapshot_bytes: 8 * 1024 * 1024,
			original_bytes: default_original_bytes(),
		}
	}
}

#[cfg(test)]
mod tests;
