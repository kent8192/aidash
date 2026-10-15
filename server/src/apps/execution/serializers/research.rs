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

/// A request to revoke all of one Run's Research evidence (ADR 0046).
#[derive(Debug, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevocationInput {
	/// Why the evidence is revoked, 1 to 1000 characters. It is kept as audit
	/// history after the Run is deleted, so do not include URLs, page content
	/// or personal data.
	pub reason: String,
}

/// The audit record of one Evidence Revocation. It holds digests but no
/// source content.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RevocationRecord {
	pub run_id: Uuid,
	/// The Run's Tenant, when it has one.
	pub tenant: Option<String>,
	/// `subject:<tenant>/<subject>`, `deployment-administrator:<identity>` or
	/// `deployment-administrator` for the deployment's API token.
	pub revoked_by: String,
	pub reason: String,
	pub revoked_at: DateTime<Utc>,
	pub removed: RemovedCopies,
}

/// What a revocation removed, counted and identified by digest only.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RemovedCopies {
	pub sources: u32,
	pub snapshots: u32,
	pub fragments: u32,
	pub attempts: u32,
	pub invocations: u32,
	/// `sha256:` text digests of the revoked Document snapshots.
	pub snapshot_digests: Vec<String>,
}

impl From<aidash_domain::research::revocation::EvidenceRevocation> for RevocationRecord {
	fn from(record: aidash_domain::research::revocation::EvidenceRevocation) -> Self {
		let removed = record.removed;
		Self {
			run_id: record.run_id,
			tenant: record.tenant,
			revoked_by: record.revoked_by.recorded(),
			reason: record.reason,
			revoked_at: record.revoked_at,
			removed: RemovedCopies {
				sources: removed.sources,
				snapshots: removed.snapshots,
				fragments: removed.fragments,
				attempts: removed.attempts,
				invocations: removed.invocations,
				snapshot_digests: removed.snapshot_digests,
			},
		}
	}
}

/// One page of audit history, newest first.
#[derive(Debug, serde::Deserialize, JsonSchema)]
pub struct HistoryPage {
	#[serde(default)]
	pub offset: u64,
	/// At most 200; 50 when absent.
	#[serde(default = "history_limit")]
	pub limit: u32,
}

fn history_limit() -> u32 {
	50
}

/// A change of a Tenant's own Retention Rules (ADR 0045).
#[derive(Debug, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionInput {
	/// The Tenant's periods, none longer than the Deployment's; null clears
	/// them so the Deployment's periods apply again. A change applies to
	/// content already kept at the next hourly sweep.
	pub rules: Option<aidash_domain::retention::RetentionRules>,
}
