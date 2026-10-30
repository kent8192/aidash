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

/// Select the original format or a WAV rendition using the current route evidence.
pub fn thread_attachment_wav(model: &ModelConfig, media_type: &str) -> Result<bool> {
	if model.require_media_types([media_type]).is_ok()
		&& ContentPart::modality_for_media_type(media_type).is_ok()
	{
		return Ok(false);
	}
	if media_type.starts_with("audio/") && model.require_media_types(["audio/wav"]).is_ok() {
		return Ok(true);
	}
	Err(Error::Invalid(format!(
		"this model cannot read {media_type} through any current route"
	)))
}

/// Find a shared current route for explicitly selected media, converting only
/// Thread audio when needed. A deleted selection contributes a text notice.
pub fn route_selected_renditions(
	model: &ModelConfig,
	selections: &[aidash_domain::media::ModelMedia],
	parts: &[ContentPart],
) -> Option<Vec<aidash_domain::media::ModelMedia>> {
	use aidash_domain::media::ModelMedia;
	if selections.len() > 8 {
		return None;
	}
	let mut candidates = Vec::new();
	let mut part = 0;
	for (index, selection) in selections.iter().enumerate() {
		if !matches!(parts.get(part), Some(ContentPart::Text(_))) {
			return None;
		}
		part += 1;
		if matches!(
			parts.get(part),
			Some(ContentPart::Image { .. } | ContentPart::Audio { .. })
		) {
			if matches!(selection, ModelMedia::ThreadAttachment(value) if !value.wav)
				&& matches!(parts.get(part), Some(ContentPart::Audio { format, .. }) if format != "wav")
			{
				candidates.push((index, part));
			}
			part += 1;
		}
	}
	if part != parts.len() {
		return None;
	}
	let formats: Vec<_> = parts
		.iter()
		.map(|part| match part {
			ContentPart::Text(_) => ContentPart::Text(String::new()),
			ContentPart::Image { media_type, .. } => ContentPart::Image {
				media_type: media_type.clone(),
				bytes: Vec::new(),
			},
			ContentPart::Audio { format, .. } => ContentPart::Audio {
				format: format.clone(),
				bytes: Vec::new(),
			},
		})
		.collect();
	let mut choices: Vec<_> = (0..(1_usize << candidates.len())).collect();
	choices.sort_by_key(|choice| choice.count_ones());
	for choice in choices {
		let mut proposed = formats.clone();
		let mut selected = selections.to_vec();
		for (bit, (selection, part)) in candidates.iter().enumerate() {
			if choice & (1 << bit) != 0 {
				if let ContentPart::Audio { format, .. } = &mut proposed[*part] {
					*format = "wav".into();
				}
				if let ModelMedia::ThreadAttachment(value) = &mut selected[*selection] {
					value.wav = true;
				}
			}
		}
		if model.has_current_media_route_for_parts(&proposed) {
			return Some(selected);
		}
	}
	None
}
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
	let mut accepted: Vec<(i64, crate::ports::execution::media::MediaAttachment)> = Vec::new();
	let mut through_seq = None;
	let mut has_more = false;
	let request = admission_request(model, projection, json!({}), Vec::new());
	for (seq, id, headroom) in messages {
		let attachments = scope.attachments(workspace, *id).await?;
		let message_bytes = attachments
			.iter()
			.fold(0_usize, |sum, a| sum.saturating_add(a.content.len()));
		if attachments.len() > 8 || message_bytes > 8 * 1024 * 1024 {
			return Err(Error::Invalid(
				"run media input exceeds count or byte limit".into(),
			));
		}
		let old_len = accepted.len();
		let accepted_bytes = accepted.iter().fold(0_usize, |sum, (_, attachment)| {
			sum.saturating_add(attachment.content.len())
		});
		if old_len + attachments.len() > 8
			|| accepted_bytes.saturating_add(message_bytes) > 8 * 1024 * 1024
		{
			has_more = true;
			break;
		}
		for attachment in attachments {
			if attachment.size_bytes != attachment.content.len() as i64 {
				return Err(Error::Invalid("run media input size changed".into()));
			}
			if format!("{:x}", Sha256::digest(&attachment.content)) != attachment.sha256 {
				return Err(Error::Conflict("OBJECT_INTEGRITY".into()));
			}
			// Preserve the domain's MIME and signature error even if no route
			// accepts this attachment.
			ContentPart::from_media(&attachment.media_type, attachment.content.clone())?;
			accepted.push((*seq, attachment));
		}
		let choice = select_renditions(model, &accepted);
		let Some(choice) = choice else {
			accepted.truncate(old_len);
			if through_seq.is_none() {
				return Err(Error::MediaRouteUnavailable(model.model_id.clone()));
			}
			has_more = true;
			break;
		};
		let mut proposed = Vec::new();
		let mut total = 0_usize;
		for (index, (sequence, attachment)) in accepted.iter().enumerate() {
			let wav = choice & (1 << index) != 0;
			let bytes = if wav {
				scope.wav_rendition(workspace, attachment).await?
			} else {
				attachment.content.clone()
			};
			total = total.saturating_add(bytes.len());
			proposed.push(ContentPart::Text(format!(
				"Run message {sequence} attachment: {}",
				attachment.filename
			)));
			proposed.push(ContentPart::from_media(
				if wav {
					"audio/wav"
				} else {
					&attachment.media_type
				},
				bytes,
			)?);
		}
		if total > 8 * 1024 * 1024 {
			accepted.truncate(old_len);
			if through_seq.is_none() {
				return Err(Error::Invalid(
					"run media input exceeds count or byte limit".into(),
				));
			}
			has_more = true;
			break;
		}
		if let Err(error) = Reservation::check_request_with_parts(*headroom, &request, &proposed) {
			accepted.truncate(old_len);
			if through_seq.is_none() {
				return Err(error);
			}
			has_more = true;
			break;
		}
		parts = proposed;
		through_seq = Some(*seq);
	}
	Ok(HumanMediaBatch {
		parts,
		through_seq,
		has_more,
	})
}

/// Find one current route for the whole batch, preferring fewer conversions.
fn select_renditions(
	model: &ModelConfig,
	attachments: &[(i64, crate::ports::execution::media::MediaAttachment)],
) -> Option<usize> {
	let mut choices: Vec<_> = (0..(1_usize << attachments.len())).collect();
	choices.sort_by_key(|choice| choice.count_ones());
	choices.into_iter().find(|choice| {
		let mut formats = Vec::with_capacity(attachments.len());
		for (index, (_, attachment)) in attachments.iter().enumerate() {
			let wav = choice & (1 << index) != 0;
			if wav
				&& (!attachment.media_type.starts_with("audio/")
					|| matches!(attachment.media_type.as_str(), "audio/wav" | "audio/x-wav"))
			{
				return false;
			}
			formats.push(if wav {
				"audio/wav"
			} else {
				&attachment.media_type
			});
		}
		model.require_media_types(formats).is_ok()
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
			.any(|selection| matches!(selection, ModelMedia::File(_)))
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
			ModelMedia::ThreadAttachment(selection) => {
				let Some((filename, media_type, bytes)) =
					scope.thread_attachment(selection).await?
				else {
					parts.push(ContentPart::Text(format!(
						"Thread Attachment unavailable: {} was deleted.",
						selection.attachment_id
					)));
					continue;
				};
				total = total.saturating_add(bytes.len() as u64);
				if total > 8 * 1024 * 1024 {
					return Err(Error::Invalid(
						"model media input exceeds byte limit".into(),
					));
				}
				(
					format!("Selected Thread Attachment: {filename}"),
					media_type,
					bytes,
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
