//! Channel use cases preserve admission, visibility, replay, and write ordering.
use crate::{Error, Result, ports::channels::ChannelScope};
use aidash_domain::{
	nonempty,
	threads::channels::*,
	unconfirmed::{Kind, Tally},
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub struct MessageSubmission<'a> {
	pub content: &'a str,
	pub idempotency_key: Uuid,
	pub attachment_ids: &'a [Uuid],
	pub citations: &'a [CitedFileVersion],
}
pub struct HistoryRequest {
	pub before: Option<Uuid>,
	pub limit: u16,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaMeasurements {
	pub width: Option<i32>,
	pub height: Option<i32>,
	pub duration_ms: Option<i64>,
}

pub struct AttachmentUpload<'a> {
	pub filename: &'a str,
	pub media_type: &'a str,
	pub idempotency_key: Uuid,
	pub measurements: MediaMeasurements,
	/// The uploader's Image Description, if any; only an image has one.
	pub description: Option<&'a str>,
}

pub async fn post(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	input: MessageSubmission<'_>,
) -> Result<ChannelMessage> {
	validate_citations(input.citations)?;
	if input.attachment_ids.is_empty() {
		nonempty(input.content, "message")?;
	}
	let attachment_digest = digest_ids(input.attachment_ids)?;
	let sender = scope.sender();
	let key = format!(
		"channel:{thread}:{}:{}",
		scope.principal(),
		input.idempotency_key
	);
	let message = scope
		.append_message(thread, &sender, input.content, &key)
		.await?;
	// Bind the attachment digest to the message before any attachment write.
	let context = scope
		.record_context(thread, message.id, &attachment_digest)
		.await?;
	if context.attachment_digest != attachment_digest {
		let matches = if input.attachment_ids.len() < 2 {
			false
		} else {
			legacy_digest_matches(
				input.attachment_ids,
				&scope
					.legacy_attachment_positions(thread, message.id)
					.await?,
				&context.attachment_digest,
			)?
		};
		if !matches {
			return Err(Error::Conflict(
				"message idempotency key reused for different attachments".into(),
			));
		}
	}
	let citations = if input.citations.is_empty() {
		Vec::new()
	} else {
		scope
			.record_citations(thread, message.id, input.citations)
			.await?
	};
	let attachments = attach(scope, thread, message.id, input.attachment_ids).await?;
	// A posted image enters the Thread index with the message, under the
	// poster's authority (#622 decision 2).
	if attachments
		.iter()
		.any(|attachment| attachment.media_type.starts_with("image/"))
	{
		scope
			.index_attachment_images(thread, message.id, &attachments)
			.await?;
	}
	Ok(ChannelMessage {
		message,
		attachments,
		citations,
	})
}
async fn attach(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	message: Uuid,
	ids: &[Uuid],
) -> Result<Vec<ChannelAttachment>> {
	if ids.is_empty() {
		return Ok(vec![]);
	}
	let principal = scope.principal();
	let records = scope.attachment_links(thread, ids, &principal).await?;
	if records.len() != ids.len() {
		return Err(Error::NotFound("attachment unavailable".into()));
	}
	if records
		.iter()
		.any(|r| r.message_id.is_some_and(|id| id != message))
	{
		return Err(Error::Conflict(
			"attachment is already linked to another message".into(),
		));
	}
	for (position, id) in ids.iter().enumerate() {
		scope
			.bind_attachment(
				thread,
				message,
				*id,
				&principal,
				i32::try_from(position)
					.map_err(|_| Error::Invalid("too many attachments".into()))?,
			)
			.await?;
	}
	Ok(ids
		.iter()
		.map(|id| {
			records
				.iter()
				.find(|r| r.attachment.id == *id)
				.expect("queried attachment id exists")
				.attachment
				.clone()
		})
		.collect())
}
pub async fn upload(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	input: AttachmentUpload<'_>,
	content: &[u8],
) -> Result<ChannelAttachment> {
	validate_attachment(input.filename, input.media_type, content)?;
	if let Some(description) = input.description {
		validate_attachment_description(input.media_type, description)?;
	}
	let uploader = scope.principal();
	let digest = format!("{:x}", Sha256::digest(content));
	let attachment = match scope
		.insert_attachment(thread, &uploader, &input, &digest, content)
		.await?
	{
		Some(record) => record,
		None => {
			let existing = scope
				.existing_attachment(thread, &uploader, input.idempotency_key)
				.await?;
			if existing.sha256 != digest
				|| existing.filename != input.filename
				|| existing.media_type != input.media_type
				|| input
					.description
					.is_some_and(|text| existing.description.as_deref() != Some(text))
				|| attachment_bytes(scope, &existing).await? != content
			{
				return Err(Error::Conflict(
					"attachment idempotency key was reused with different content or metadata"
						.into(),
				));
			}
			existing
		}
	};
	if attachment.thread_id != thread
		|| attachment.uploaded_by != uploader
		|| attachment.idempotency_key != input.idempotency_key
	{
		return Err(Error::Conflict(
			"attachment idempotency scope changed".into(),
		));
	}
	Ok(attachment.metadata())
}
pub async fn attachment_bytes(
	scope: &mut dyn ChannelScope,
	attachment: &AttachmentState,
) -> Result<Vec<u8>> {
	let bytes = if let Some(id) = attachment.object_id {
		scope
			.read_object(id, &attachment.sha256, attachment.size_bytes)
			.await?
	} else {
		attachment
			.content
			.clone()
			.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?
	};
	if i64::try_from(bytes.len()).ok() != Some(attachment.size_bytes)
		|| format!("{:x}", Sha256::digest(&bytes)) != attachment.sha256
	{
		return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
	}
	Ok(bytes)
}
/// Write or replace an image's Image Description. Only the person or Agent who
/// uploaded the image writes it (#622 decision 7), before or after posting it;
/// an indexed image follows.
pub async fn describe(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	id: Uuid,
	description: &str,
) -> Result<ChannelAttachment> {
	let attachment = scope
		.attachment(thread, id)
		.await?
		.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	// A posted image is described only by a writer who can still read its
	// message; an unposted upload is visible to its uploader alone.
	if let Some(message) = attachment.message_id {
		scope.message(thread, message).await?;
	}
	if attachment.uploaded_by != scope.principal() {
		return Err(if attachment.message_id.is_some() {
			Error::Forbidden
		} else {
			Error::NotFound("attachment unavailable".into())
		});
	}
	validate_attachment_description(&attachment.media_type, description)?;
	if attachment.description.as_deref() == Some(description) {
		return Ok(attachment.metadata());
	}
	scope
		.describe_attachment(thread, id, &attachment.uploaded_by, description)
		.await?;
	let mut metadata = attachment.metadata();
	metadata.description = Some(description.to_owned());
	Ok(metadata)
}
pub async fn authorize_download(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	id: Uuid,
) -> Result<(AttachmentState, bool)> {
	let attachment = scope
		.attachment(thread, id)
		.await?
		.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	let message = attachment
		.message_id
		.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	let message = scope.message(thread, message).await?;
	Ok((attachment, message.completion.is_some()))
}

pub async fn download(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	id: Uuid,
) -> Result<(ChannelAttachment, Vec<u8>)> {
	let (attachment, _) = authorize_download(scope, thread, id).await?;
	let content = attachment_bytes(scope, &attachment).await?;
	Ok((attachment.metadata(), content))
}

pub async fn history(
	scope: &mut dyn ChannelScope,
	thread: Uuid,
	input: HistoryRequest,
) -> Result<ChannelMessagePage> {
	if !(1..=100).contains(&input.limit) {
		return Err(Error::Invalid(
			"history limit must be between 1 and 100".into(),
		));
	}
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = match input.before {
		Some(id) => {
			let message = scope.message(thread, id).await?;
			Some((message.created_at, message.id))
		}
		None => None,
	};
	let mut result = Vec::new();
	let mut unconfirmed = Tally::new();
	// What had been counted when the page's last message was found. The next
	// page resumes after that message, so rows scanned beyond it are counted
	// there, not here (#770).
	let mut at_cut = Tally::new();
	let mut scanned = 0;
	let more = loop {
		let rows = scope.history_rows(thread, cursor).await?;
		let exhausted = rows.len() < 100;
		for row in rows {
			scanned += 1;
			cursor = Some((row.message.created_at, row.message.id));
			scope.begin_row();
			let visible = scope.visible(&row.message).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle(silence, Kind::Message, thread, visible) {
				result.push(ChannelMessage {
					message: row.message,
					attachments: Vec::new(),
					citations: Vec::new(),
				});
				if result.len() == usize::from(input.limit) {
					at_cut = unconfirmed.clone();
				}
				if result.len() > usize::from(input.limit) {
					break;
				}
			}
		}
		if result.len() > usize::from(input.limit) {
			break true;
		}
		if exhausted {
			break false;
		}
		// Hidden identifiers never become pagination cursors. Fail explicitly
		// rather than claim completeness after a bounded authorization scan.
		if scanned >= 5000 {
			return Err(Error::Invalid(
				"history scan limit reached; page with an older cursor".into(),
			));
		}
	};
	if more {
		result.pop();
		unconfirmed = at_cut;
	}
	let next_before = if more {
		result.last().map(|item| item.message.id)
	} else {
		None
	};
	result.reverse();
	let message_ids: Vec<_> = result.iter().map(|item| item.message.id).collect();
	let mut message_attachments = scope.message_attachments(thread, &message_ids).await?;
	let mut citations = scope.message_citations(thread, &message_ids).await?;
	for item in &mut result {
		item.attachments = message_attachments
			.remove(&item.message.id)
			.unwrap_or_default();
		item.citations = citations.remove(&item.message.id).unwrap_or_default();
	}
	Ok(ChannelMessagePage {
		messages: result,
		next_before,
		unconfirmed: unconfirmed.into_entries(),
	})
}

#[cfg(test)]
mod tests;
