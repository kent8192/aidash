//! Channel use cases preserve admission, visibility, replay, and write ordering.
use crate::{Error, Result, ports::channels::ChannelScope};
use aidash_domain::{nonempty, workspaces::channels::*};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub struct MessageSubmission<'a> {
	pub content: &'a str,
	pub thread_id: Option<Uuid>,
	pub idempotency_key: Uuid,
	pub attachment_ids: &'a [Uuid],
}
pub struct HistoryRequest {
	pub thread_id: Option<Uuid>,
	pub before: Option<Uuid>,
	pub limit: u16,
}
pub struct AttachmentUpload<'a> {
	pub filename: &'a str,
	pub media_type: &'a str,
	pub idempotency_key: Uuid,
}

pub async fn thread(
	scope: &mut dyn ChannelScope,
	workspace: Uuid,
	id: Uuid,
) -> Result<ChannelThread> {
	scope.thread_visible(id).await?;
	let thread = scope
		.thread(workspace, id)
		.await?
		.ok_or_else(|| Error::NotFound("thread unavailable".into()))?;
	scope.message(workspace, thread.root_message_id).await?;
	Ok(thread)
}
pub async fn create_thread(
	scope: &mut dyn ChannelScope,
	workspace: Uuid,
	root: Uuid,
) -> Result<ChannelThread> {
	scope.message(workspace, root).await?;
	if scope.reply_thread(root).await?.is_some() {
		return Err(Error::Conflict(
			"a reply already belongs to a thread".into(),
		));
	}
	let sender = scope.sender();
	if let Some(thread) = scope.insert_thread(workspace, root, &sender).await? {
		scope.thread_event(workspace, root, thread.id).await?;
		return Ok(thread);
	}
	let existing = scope.existing_thread(workspace, root).await?;
	scope.thread_visible(existing.id).await?;
	Ok(existing)
}
pub async fn post(
	scope: &mut dyn ChannelScope,
	workspace: Uuid,
	input: MessageSubmission<'_>,
) -> Result<ChannelMessage> {
	if input.attachment_ids.is_empty() {
		nonempty(input.content, "message")?;
	}
	let attachment_digest = digest_ids(input.attachment_ids)?;
	if let Some(id) = input.thread_id {
		thread(scope, workspace, id).await?;
	}
	let sender = scope.sender();
	let key = format!(
		"channel:{workspace}:{}:{}",
		scope.principal(),
		input.idempotency_key
	);
	let message = scope
		.append_message(workspace, &sender, input.content, &key)
		.await?;
	// Bind root submissions to the absence of a thread before any attachment write.
	let context = scope
		.record_context(workspace, message.id, input.thread_id, &attachment_digest)
		.await?;
	if context.thread_id != input.thread_id {
		return Err(Error::Conflict(
			"message idempotency key reused for a different thread".into(),
		));
	}
	if context.attachment_digest != attachment_digest {
		let matches = if input.attachment_ids.len() < 2 {
			false
		} else {
			legacy_digest_matches(
				input.attachment_ids,
				&scope
					.legacy_attachment_positions(workspace, message.id)
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
	let attachments = attach(scope, workspace, message.id, input.attachment_ids).await?;
	let root_thread = scope.root_thread(message.id).await?;
	Ok(ChannelMessage {
		message,
		thread_id: context.thread_id.or(root_thread),
		is_thread_root: root_thread.is_some(),
		attachments,
	})
}
async fn attach(
	scope: &mut dyn ChannelScope,
	workspace: Uuid,
	message: Uuid,
	ids: &[Uuid],
) -> Result<Vec<ChannelAttachment>> {
	if ids.is_empty() {
		return Ok(vec![]);
	}
	let principal = scope.principal();
	let records = scope.attachment_links(workspace, ids, &principal).await?;
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
				workspace,
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
	workspace: Uuid,
	input: AttachmentUpload<'_>,
	content: &[u8],
) -> Result<ChannelAttachment> {
	validate_attachment(input.filename, input.media_type, content)?;
	let uploader = scope.principal();
	let digest = format!("{:x}", Sha256::digest(content));
	let attachment = match scope
		.insert_attachment(workspace, &uploader, &input, &digest, content)
		.await?
	{
		Some(record) => record,
		None => {
			let existing = scope
				.existing_attachment(workspace, &uploader, input.idempotency_key)
				.await?;
			if existing.sha256 != digest
				|| existing.filename != input.filename
				|| existing.media_type != input.media_type
				|| existing.content != content
			{
				return Err(Error::Conflict(
					"attachment idempotency key was reused with different content or metadata"
						.into(),
				));
			}
			existing
		}
	};
	if attachment.workspace_id != workspace
		|| attachment.uploaded_by != uploader
		|| attachment.idempotency_key != input.idempotency_key
	{
		return Err(Error::Conflict(
			"attachment idempotency scope changed".into(),
		));
	}
	Ok(attachment.metadata())
}
pub async fn download(
	scope: &mut dyn ChannelScope,
	workspace: Uuid,
	id: Uuid,
) -> Result<(ChannelAttachment, Vec<u8>)> {
	let attachment = scope
		.attachment(workspace, id)
		.await?
		.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	let message = attachment
		.message_id
		.ok_or_else(|| Error::NotFound("attachment unavailable".into()))?;
	scope.message(workspace, message).await?;
	Ok((attachment.metadata(), attachment.content))
}

pub async fn history(
	scope: &mut dyn ChannelScope,
	workspace: Uuid,
	input: HistoryRequest,
) -> Result<ChannelMessagePage> {
	if !(1..=100).contains(&input.limit) {
		return Err(Error::Invalid(
			"history limit must be between 1 and 100".into(),
		));
	}
	let thread = match input.thread_id {
		Some(id) => Some(thread(scope, workspace, id).await?),
		None => None,
	};
	let mut cursor: Option<(DateTime<Utc>, Uuid)> = match input.before {
		Some(id) => {
			let message = scope.message(workspace, id).await?;
			let reply = scope.reply_thread(id).await?;
			let valid = match &thread {
				Some(thread) => id == thread.root_message_id || reply == Some(thread.id),
				None => reply.is_none(),
			};
			if !valid {
				return Err(Error::NotFound("message unavailable".into()));
			}
			Some((message.created_at, message.id))
		}
		None => None,
	};
	let mut result = Vec::new();
	let mut scanned = 0;
	let more = loop {
		let rows = scope
			.history_rows(workspace, thread.as_ref(), cursor)
			.await?;
		let exhausted = rows.len() < 100;
		for row in rows {
			scanned += 1;
			cursor = Some((row.message.created_at, row.message.id));
			if scope.visible(&row.message).await? {
				result.push(ChannelMessage {
					message: row.message,
					thread_id: row.reply_thread_id.or(row.root_thread_id),
					is_thread_root: row.root_thread_id.is_some(),
					attachments: Vec::new(),
				});
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
				"history scan limit reached; select a narrower thread".into(),
			));
		}
	};
	if more {
		result.pop();
	}
	let next_before = if more {
		result.last().map(|item| item.message.id)
	} else {
		None
	};
	result.reverse();
	let message_ids: Vec<_> = result.iter().map(|item| item.message.id).collect();
	let mut message_attachments = scope.message_attachments(workspace, &message_ids).await?;
	for item in &mut result {
		item.attachments = message_attachments
			.remove(&item.message.id)
			.unwrap_or_default();
	}
	Ok(ChannelMessagePage {
		messages: result,
		next_before,
	})
}

#[cfg(test)]
mod tests;
