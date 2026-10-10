//! Ordered media batches preserve message boundaries, integrity, budgets and exact route approval.
use crate::{
	Error, Result,
	generation::inference::Reservation,
	ports::execution::{
		HumanMediaBatch,
		media::{AuthorizedMediaScope, MediaAttachments, OperatorMediaRepository},
	},
};
use aidash_domain::{
	RunMetadata,
	model::ModelConfig,
	provider::{ContentPart, ModelRequest},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
pub async fn operator(
	repository: &dyn OperatorMediaRepository,
	run: &RunMetadata,
	messages: &[(i64, Uuid, usize)],
	model: &ModelConfig,
) -> Result<HumanMediaBatch> {
	if run.home_node != repository.node_id() {
		return Err(Error::Forbidden);
	}
	let mut transaction = repository.begin().await?;
	let batch = load(transaction.as_mut(), run.workspace_id, messages, model).await?;
	transaction.commit().await?;
	Ok(batch)
}
pub async fn authorized<S: AuthorizedMediaScope + ?Sized>(
	scope: &mut S,
	workspace: Uuid,
	messages: &[(i64, Uuid, usize)],
	model: &ModelConfig,
) -> Result<HumanMediaBatch> {
	for (_, message, _) in messages {
		scope.authorize_message(workspace, *message).await?;
	}
	load(scope, workspace, messages, model).await
}
pub async fn load<S: MediaAttachments + ?Sized>(
	scope: &mut S,
	workspace: Uuid,
	messages: &[(i64, Uuid, usize)],
	model: &ModelConfig,
) -> Result<HumanMediaBatch> {
	let mut parts = Vec::new();
	let mut count = 0_usize;
	let mut total = 0_usize;
	let mut through_seq = None;
	let mut has_more = false;
	let request = ModelRequest {
		instructions: String::new(),
		context: json!({}).into(),
		tools: Vec::new(),
		max_output_tokens: 0,
		response_format: None,
		content_parts: Vec::new(),
		cache_scope: None,
	};
	for (seq, id, headroom) in messages {
		let attachments = scope.attachments(workspace, *id).await?;
		let message_bytes = attachments.iter().fold(0_usize, |sum, attachment| {
			sum.saturating_add(attachment.content.len())
		});
		if attachments.len() > 8 || message_bytes > 8 * 1024 * 1024 {
			return Err(Error::Invalid(
				"run media input exceeds count or byte limit".into(),
			));
		}
		if count + attachments.len() > 8 || total.saturating_add(message_bytes) > 8 * 1024 * 1024 {
			has_more = true;
			break;
		}
		let previous_len = parts.len();
		let attachment_count = attachments.len();
		for attachment in attachments {
			if attachment.size_bytes != attachment.content.len() as i64 {
				return Err(Error::Invalid("run media input size changed".into()));
			}
			if format!("{:x}", Sha256::digest(&attachment.content)) != attachment.sha256 {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			parts.push(ContentPart::Text(format!(
				"Run message {seq} attachment: {}",
				attachment.filename
			)));
			parts.push(ContentPart::from_media(
				&attachment.media_type,
				attachment.content,
			)?);
		}
		if let Err(error) = Reservation::check_request_with_parts(*headroom, &request, &parts) {
			parts.truncate(previous_len);
			if through_seq.is_none() {
				return Err(error);
			}
			has_more = true;
			break;
		}
		if !model.has_current_media_route_for_parts(&parts) {
			parts.truncate(previous_len);
			if through_seq.is_none() {
				return Err(Error::MediaRouteUnavailable(model.model_id.clone()));
			}
			has_more = true;
			break;
		}
		count += attachment_count;
		total = total.saturating_add(message_bytes);
		through_seq = Some(*seq);
	}
	Ok(HumanMediaBatch {
		parts,
		through_seq,
		has_more,
	})
}

/// Resolve one inference's explicit file references under its current execution authority.
pub async fn selected(
	scope: &mut dyn crate::ports::execution::media::SelectedMediaScope,
	selections: &[aidash_domain::media::Selection],
) -> Result<Vec<ContentPart>> {
	if selections.len() > 8 {
		return Err(Error::Invalid(
			"model media input exceeds count limit".into(),
		));
	}
	let files = scope.current_files().await?;
	let mut parts = Vec::with_capacity(selections.len() * 2);
	let mut total = 0_u64;
	for selection in selections {
		let file = files
			.iter()
			.find(|file| file.file_id == selection.file_id)
			.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
		if file.digest != selection.expected_digest {
			return Err(Error::Conflict("FILE_CHANGED".into()));
		}
		total = total.saturating_add(file.size);
		if total > 8 * 1024 * 1024 {
			return Err(Error::Invalid(
				"model media input exceeds byte limit".into(),
			));
		}
		let bytes = scope.read(file.file_id).await?;
		parts.push(ContentPart::Text(format!("Selected file: {}", file.path)));
		parts.push(ContentPart::from_media(&file.media_type, bytes)?);
	}
	Ok(parts)
}

#[cfg(test)]
mod tests;
