//! Research evidence read contracts.
use aidash_domain::research::{
	document::{Completeness, Line},
	evidence::{CitedEvidence as Cited, SnapshotContext},
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use uuid::Uuid;

/// One Source citation as a reader opens it: the Evidence fragment the Run's
/// Agent received and the Document snapshot it came from.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CitedEvidence {
	/// `ev_<uuid>`, as cited in the Run's text.
	pub evidence_ref: String,
	pub run_id: Uuid,
	pub document_id: Uuid,
	pub source_id: Uuid,
	/// The address that answered, after any permitted redirect.
	pub url: String,
	/// The address the Run asked for.
	pub requested_url: String,
	pub title: Option<String>,
	pub media_type: String,
	/// When the snapshot was retrieved; the live page may have changed since.
	pub fetched_at: DateTime<Utc>,
	pub completeness: Completeness,
	/// `sha256:` digest of the snapshot text the fragment was taken from.
	pub text_digest: String,
	/// The passage exactly as delivered to the Agent.
	pub text: String,
	/// 1-based, inclusive snapshot lines of the passage.
	pub line_start: usize,
	pub line_end: usize,
	pub pages: Vec<u32>,
	/// Snapshot lines around the passage that were not delivered to the Agent;
	/// absent once the snapshot's text is no longer retained.
	pub context: Option<EvidenceContext>,
}

/// Up to 20 whole lines and 4 KiB of snapshot text on each side of a passage.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EvidenceContext {
	pub before: Vec<Line>,
	pub after: Vec<Line>,
}

impl From<SnapshotContext> for EvidenceContext {
	fn from(context: SnapshotContext) -> Self {
		Self {
			before: context.before,
			after: context.after,
		}
	}
}

impl From<Cited> for CitedEvidence {
	fn from(cited: Cited) -> Self {
		let fragment = cited.fragment;
		Self {
			evidence_ref: fragment.reference.to_string(),
			run_id: fragment.run_id,
			document_id: fragment.document_id,
			source_id: fragment.source_id,
			url: cited.url,
			requested_url: cited.requested_url,
			title: cited.title,
			media_type: cited.media_type,
			fetched_at: cited.fetched_at,
			completeness: cited.completeness,
			text_digest: fragment.text_digest,
			text: fragment.text,
			line_start: fragment.line_start,
			line_end: fragment.line_end,
			pages: fragment.pages,
			context: cited.context.map(Into::into),
		}
	}
}
