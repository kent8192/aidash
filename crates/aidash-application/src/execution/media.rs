//! Ordered media batches preserve message boundaries, integrity, budgets and exact route approval.
use crate::{
	Error, Result,
	generation::inference::Reservation,
	ports::execution::{
		HumanMediaBatch,
		media::{AuthorizedMediaScope, MediaAttachments},
	},
};
use aidash_domain::{
	model::ModelConfig,
	projection::ProjectionVersion,
	provider::{ContentPart, InputFormat, ModelContext, ModelRequest},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
/// Conservative media pre-admission in the selected provider input shape.
/// Native plan labels use the same provenance envelopes as the final request.
/// Other routes retain their established Chat probe for compatibility.
pub fn admission_request(
	model: &ModelConfig,
	projection: ProjectionVersion,
	context: serde_json::Value,
	content_parts: Vec<ContentPart>,
) -> ModelRequest {
	let input_format = InputFormat::for_model(model, projection);
	let context = if input_format == InputFormat::Responses {
		ModelContext::Native(aidash_domain::projection::native::native_context(
			&Default::default(),
			&context,
		))
	} else {
		context.into()
	};
	ModelRequest {
		context,
		content_parts,
		input_format,
		..Default::default()
	}
}

pub async fn authorized<S: AuthorizedMediaScope + ?Sized>(
	scope: &mut S,
	workspace: Uuid,
	messages: &[(i64, Uuid, usize)],
	model: &ModelConfig,
	projection: ProjectionVersion,
) -> Result<HumanMediaBatch> {
	for (_, message, _) in messages {
		scope.authorize_message(workspace, *message).await?;
	}
	load(scope, workspace, messages, model, projection).await
}
pub async fn load<S: MediaAttachments + ?Sized>(
	scope: &mut S,
	workspace: Uuid,
	messages: &[(i64, Uuid, usize)],
	model: &ModelConfig,
	projection: ProjectionVersion,
) -> Result<HumanMediaBatch> {
	let mut parts = Vec::new();
	let mut count = 0_usize;
	let mut total = 0_usize;
	let mut through_seq = None;
	let mut has_more = false;
	let request = admission_request(model, projection, json!({}), Vec::new());
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

/// Resolve one inference's explicit file references and page images under its
/// current execution authority. A request of page images alone, the Run's own
/// evidence, needs no file access (ADR 0053).
pub async fn selected(
	scope: &mut dyn crate::ports::execution::media::SelectedMediaScope,
	selections: &[aidash_domain::media::ModelMedia],
) -> Result<Vec<ContentPart>> {
	use aidash_domain::media::ModelMedia;
	if selections.len() > 8 {
		return Err(Error::Invalid(
			"model media input exceeds count limit".into(),
		));
	}
	// Only page images, the Run's own evidence, skip the file authority; an
	// empty request still requires it.
	let files = if selections.is_empty()
		|| selections
			.iter()
			.any(|selection| matches!(selection, ModelMedia::File(_) | ModelMedia::WorkingImage(_)))
	{
		scope.current_files().await?
	} else {
		Vec::new()
	};
	let mut parts = Vec::with_capacity(selections.len() * 2);
	let mut total = 0_u64;
	for selection in selections {
		let (label, media_type, bytes) = match selection {
			ModelMedia::File(selection) => {
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
				(
					format!("Selected file: {}", file.path),
					file.media_type.clone(),
					scope.read(file.file_id).await?,
				)
			}
			ModelMedia::WorkingImage(selection) => {
				let file = scope
					.workspace_image(
						selection.area_id,
						selection.generation,
						selection.file_id,
						&selection.path,
					)
					.await?
					.ok_or_else(|| Error::NotFound("file unavailable".into()))?;
				if file.digest != selection.expected_digest {
					return Err(Error::Conflict("FILE_CHANGED".into()));
				}
				if !file.media_type.starts_with("image/") {
					return Err(Error::Invalid("selected File is not an image".into()));
				}
				total = total.saturating_add(file.size);
				if total > 8 * 1024 * 1024 {
					return Err(Error::Invalid(
						"model media input exceeds byte limit".into(),
					));
				}
				(
					format!("Selected file: {}", file.path),
					file.media_type,
					scope.read(file.file_id).await?,
				)
			}
			ModelMedia::PageImage(selection) => {
				// A revoked page leaves the Run's durable selection behind;
				// failing on it would stop the Run on every retry, so the
				// model is told the page is gone instead.
				let Some((media_type, bytes)) = scope.page_image(&selection.evidence_ref).await?
				else {
					parts.push(ContentPart::Text(format!(
						"Page image unavailable: {} was revoked.",
						selection.evidence_ref
					)));
					continue;
				};
				if format!("sha256:{:x}", Sha256::digest(&bytes)) != selection.expected_digest {
					return Err(Error::Conflict("PAGE_IMAGE_CHANGED".into()));
				}
				total = total.saturating_add(bytes.len() as u64);
				if total > 8 * 1024 * 1024 {
					return Err(Error::Invalid(
						"model media input exceeds byte limit".into(),
					));
				}
				(
					format!("Page image: {}", selection.evidence_ref),
					media_type,
					bytes,
				)
			}
		};
		parts.push(ContentPart::Text(label));
		parts.push(ContentPart::from_media(&media_type, bytes)?);
	}
	Ok(parts)
}

#[cfg(test)]
mod tests;
