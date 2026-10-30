//! Durable channel contracts, attachment identities, and replay invariants.
use crate::{Error, Message, Result};
use chrono::{DateTime, Utc};
use http::HeaderValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelThread {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub root_message_id: Uuid,
	pub created_by: String,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelMessage {
	pub message: Message,
	pub thread_id: Option<Uuid>,
	pub is_thread_root: bool,
	pub attachments: Vec<ChannelAttachment>,
	#[serde(default)]
	pub citations: Vec<ChannelCitation>,
}

/// One exact File version a message cites. The ids are what the composer sends;
/// the Working Area revision and the version metadata are recorded at cite time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CitedFileVersion {
	pub area_id: Uuid,
	pub file_id: Uuid,
	/// Source Run for a File version hosted on another Node.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub run_id: Option<Uuid>,
}

/// A citation as one viewer may see it.
///
/// `available` is false when the hold that kept the version is gone; the version
/// is never substituted. A viewer who may not download the Working Area sees the
/// path and ids only, with `reason = "forbidden"` and no digest, size, media type
/// or revision.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelCitation {
	pub area_id: Uuid,
	pub file_id: Uuid,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub run_id: Option<Uuid>,
	pub path: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub digest: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub size: Option<u64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub media_type: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub revision: Option<i64>,
	pub available: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub reason: Option<String>,
}

/// One message cites at most this many File versions.
pub const MAX_CITED_VERSIONS: usize = 8;

/// Reject an oversized or duplicated citation list before any write.
pub fn validate_citations(citations: &[CitedFileVersion]) -> Result<()> {
	if citations.len() > MAX_CITED_VERSIONS {
		return Err(Error::Invalid(
			"CITATION_LIMIT: at most 8 File versions".into(),
		));
	}
	let mut seen = HashSet::new();
	for citation in citations {
		if !seen.insert((citation.area_id, citation.file_id)) {
			return Err(Error::Invalid(
				"CITATION_DUPLICATE: the same File version is cited twice".into(),
			));
		}
	}
	Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelAttachment {
	pub id: Uuid,
	pub filename: String,
	pub media_type: String,
	pub size_bytes: i64,
	/// Computed for the Workspace's currently assigned Agents, not persisted.
	#[serde(default)]
	pub no_assigned_agent_can_read: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChannelMessagePage {
	/// Each page is chronological; next_before retrieves an older page.
	pub messages: Vec<ChannelMessage>,
	pub next_before: Option<Uuid>,
}

pub fn validate_attachment(filename: &str, media_type: &str, content: &[u8]) -> Result<()> {
	if content.is_empty() {
		return Err(Error::Invalid("attachment must not be empty".into()));
	}
	if content.len() > 8 * 1024 * 1024 {
		return Err(Error::Invalid("attachment exceeds 8 MiB".into()));
	}
	if filename.is_empty()
		|| filename.len() > 255
		|| filename == "."
		|| filename == ".."
		|| filename.contains('/')
		|| filename.contains('\\')
		|| filename.chars().any(char::is_control)
	{
		return Err(Error::Invalid("invalid attachment filename".into()));
	}
	if media_type.len() > 128
		|| !media_type.contains('/')
		|| HeaderValue::from_str(media_type).is_err()
	{
		return Err(Error::Invalid("invalid attachment media type".into()));
	}
	Ok(())
}

pub fn digest_ids(ids: &[Uuid]) -> Result<String> {
	let unique: HashSet<_> = ids.iter().copied().collect();
	if unique.len() != ids.len() {
		return Err(Error::Invalid("attachment ids must be unique".into()));
	}
	let canonical: Vec<_> = ids.iter().map(Uuid::to_string).collect();
	Ok(format!(
		"{:x}",
		Sha256::digest(canonical.join("\n").as_bytes())
	))
}

/// Persisted attachment identity; independent of the database representation.
#[derive(Debug, Clone)]
pub struct AttachmentState {
	pub id: Uuid,
	pub workspace_id: Uuid,
	pub uploaded_by: String,
	pub idempotency_key: Uuid,
	pub filename: String,
	pub media_type: String,
	pub sha256: String,
	pub size_bytes: i64,
	pub content: Option<Vec<u8>>,
	pub object_id: Option<Uuid>,
	pub message_id: Option<Uuid>,
}
impl AttachmentState {
	pub fn metadata(&self) -> ChannelAttachment {
		ChannelAttachment {
			id: self.id,
			filename: self.filename.clone(),
			media_type: self.media_type.clone(),
			size_bytes: self.size_bytes,
			no_assigned_agent_can_read: false,
		}
	}
}

/// Older message records used a sorted digest and had no attachment positions.
pub fn legacy_digest_matches(ids: &[Uuid], rows: &[(Uuid, i32)], digest: &str) -> Result<bool> {
	if ids.len() < 2 || rows.len() != ids.len() || rows.iter().any(|(_, p)| *p != 0) {
		return Ok(false);
	}
	let mut prior: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
	let mut retried = ids.to_vec();
	prior.sort_unstable();
	retried.sort_unstable();
	Ok(prior == retried && digest_ids(&retried)? == digest)
}

#[cfg(test)]
mod tests;
