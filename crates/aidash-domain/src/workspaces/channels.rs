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
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelAttachment {
	pub id: Uuid,
	pub filename: String,
	pub media_type: String,
	pub size_bytes: i64,
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
	if content.len() > 1024 * 1024 {
		return Err(Error::Invalid("attachment exceeds 1 MiB".into()));
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
	pub content: Vec<u8>,
	pub message_id: Option<Uuid>,
}
impl AttachmentState {
	pub fn metadata(&self) -> ChannelAttachment {
		ChannelAttachment {
			id: self.id,
			filename: self.filename.clone(),
			media_type: self.media_type.clone(),
			size_bytes: self.size_bytes,
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
