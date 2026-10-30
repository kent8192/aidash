//! Attachment transport inputs adapt to the channel use cases.
//! Media decoding runs in bounded blocking workers: supported pure-Rust codecs
//! have explicit input, image-allocation and decoded-duration limits, unlike
//! the PDF adapter whose unbounded renderer requires a child process. A timeout
//! bounds the response but cannot stop an already running blocking task; the
//! semaphore keeps such tasks from accumulating without limit.
use super::{ChannelAttachment, ChannelAttachmentUploadQuery, access::Lease};
use crate::{Result, store::Store};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use uuid::Uuid;

async fn decode<T: Send + 'static>(
	work: impl FnOnce() -> std::result::Result<T, aidash_integrations::media::Refusal> + Send + 'static,
) -> Result<T> {
	decode_bounded(std::time::Duration::from_secs(20), work).await
}

async fn decode_bounded<T: Send + 'static>(
	deadline: std::time::Duration,
	work: impl FnOnce() -> std::result::Result<T, aidash_integrations::media::Refusal> + Send + 'static,
) -> Result<T> {
	static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
	let slots = SLOTS.get_or_init(|| Arc::new(Semaphore::new(2))).clone();
	let run = async move {
		let permit = slots
			.acquire_owned()
			.await
			.map_err(|_| crate::Error::Invalid("MEDIA_UNDECODABLE".into()))?;
		tokio::task::spawn_blocking(move || {
			let _permit = permit;
			work()
		})
		.await
		.map_err(|error| {
			tracing::warn!(%error, "media decoder worker failed");
			crate::Error::Invalid("MEDIA_UNDECODABLE".into())
		})?
		.map_err(|refusal| match refusal {
			aidash_integrations::media::Refusal::Undecodable => {
				crate::Error::Invalid("MEDIA_UNDECODABLE".into())
			}
			aidash_integrations::media::Refusal::TooLarge => {
				crate::Error::Invalid("MEDIA_TOO_LARGE".into())
			}
			aidash_integrations::media::Refusal::RenditionTooLong => {
				crate::Error::MediaRouteUnavailable(
					"audio is too long to convert to WAV within the inference byte limit".into(),
				)
			}
		})
	};
	tokio::time::timeout(deadline, run)
		.await
		.unwrap_or_else(|_| {
			tracing::warn!("media decoder exceeded its deadline");
			Err(crate::Error::Invalid("MEDIA_UNDECODABLE".into()))
		})
}

pub(crate) async fn render_wav(media_type: &str, bytes: &[u8]) -> Result<Vec<u8>> {
	let media_type = media_type.to_owned();
	let bytes = bytes.to_vec();
	decode(move || aidash_integrations::media::render_wav(&media_type, &bytes)).await
}
pub(crate) async fn upload(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	input: ChannelAttachmentUploadQuery,
	content: &[u8],
) -> Result<ChannelAttachment> {
	aidash_domain::workspaces::channels::validate_attachment(
		&input.filename,
		&input.media_type,
		content,
	)?;
	let media_type = input.media_type.clone();
	let bytes = content.to_vec();
	let measured =
		decode(move || aidash_integrations::media::validate(&media_type, &bytes)).await?;
	aidash_application::workspaces::channels::upload(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		aidash_application::workspaces::channels::AttachmentUpload {
			filename: &input.filename,
			media_type: &input.media_type,
			idempotency_key: input.idempotency_key,
			measurements: aidash_application::workspaces::channels::MediaMeasurements {
				width: measured.width,
				height: measured.height,
				duration_ms: measured.duration_ms,
			},
		},
		content,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn download(
	store: &Store,
	lease: &mut Lease,
	workspace: Uuid,
	id: Uuid,
) -> Result<(ChannelAttachment, Vec<u8>)> {
	aidash_application::workspaces::channels::download(
		&mut crate::bootstrap::channel_scope(store, lease),
		workspace,
		id,
	)
	.await
	.map_err(Into::into)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[tokio::test]
	async fn failed_or_timed_out_worker_is_an_undecodable_error() {
		let panic = decode(
			|| -> std::result::Result<(), aidash_integrations::media::Refusal> {
				panic!("decoder panic")
			},
		)
		.await;
		assert!(matches!(panic, Err(crate::Error::Invalid(code)) if code == "MEDIA_UNDECODABLE"));
		let timeout = decode_bounded(std::time::Duration::from_millis(10), || {
			std::thread::sleep(std::time::Duration::from_millis(100));
			Ok(())
		})
		.await;
		assert!(matches!(timeout, Err(crate::Error::Invalid(code)) if code == "MEDIA_UNDECODABLE"));
	}
}
