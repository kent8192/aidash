//! Attachment transport inputs adapt to the channel use cases.
//! Media decoding runs in bounded blocking workers: supported pure-Rust codecs
//! have explicit input, image-allocation and decoded-duration limits, unlike
//! the PDF adapter whose unbounded renderer requires a child process. A timeout
//! bounds the response but cannot stop an already running blocking task; the
//! semaphore keeps such tasks from accumulating without limit.
use super::{ChannelAttachment, ChannelAttachmentUploadQuery, access::Lease};
use crate::{Result, store::Store};
use std::{
	sync::{Arc, OnceLock},
	time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

// One decoder per available CPU protects the process from oversubscribing blocking work.
static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
// Bound the queue wait to give the caller a retryable answer under load.
const SLOT_WAIT: Duration = Duration::from_secs(10);
// Bound the response time of a decode once its worker starts.
const DECODE_DEADLINE: Duration = Duration::from_secs(20);

async fn decode<T: Send + 'static>(
	work: impl FnOnce() -> std::result::Result<T, aidash_integrations::media::Refusal> + Send + 'static,
) -> Result<T> {
	let slots = SLOTS.get_or_init(|| {
		Arc::new(Semaphore::new(
			std::thread::available_parallelism().map_or(1, usize::from),
		))
	});
	decode_bounded(slots.clone(), DECODE_DEADLINE, work).await
}

async fn decode_bounded<T: Send + 'static>(
	slots: Arc<Semaphore>,
	deadline: Duration,
	work: impl FnOnce() -> std::result::Result<T, aidash_integrations::media::Refusal> + Send + 'static,
) -> Result<T> {
	let permit = tokio::time::timeout(SLOT_WAIT, slots.acquire_owned())
		.await
		.map_err(|_| crate::Error::MediaDecoderBusy)?
		.map_err(|_| crate::Error::External("media decoder semaphore closed".into()))?;
	let (started, ready) = tokio::sync::oneshot::channel();
	let worker = tokio::task::spawn_blocking(move || {
		let _permit = permit;
		let _ = started.send(tokio::time::Instant::now());
		work()
	});
	let started_at = ready
		.await
		.map_err(|_| crate::Error::External("media decoder worker did not start".into()))?;
	tokio::time::timeout_at(started_at + deadline, worker)
		.await
		.map_err(|_| crate::Error::MediaDecodeTimeout)?
		.map_err(|error| crate::Error::External(format!("media decoder worker failed: {error}")))?
		.map_err(|refusal| match refusal {
			aidash_integrations::media::Refusal::Undecodable => {
				crate::Error::Invalid("MEDIA_UNDECODABLE".into())
			}
			aidash_integrations::media::Refusal::TooLarge => {
				crate::Error::Invalid("MEDIA_TOO_LARGE".into())
			}
			aidash_integrations::media::Refusal::RenditionTooLong => crate::Error::Invalid(
				"audio is too long to convert to WAV within the inference byte limit".into(),
			),
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
	thread: Uuid,
	input: ChannelAttachmentUploadQuery,
	content: &[u8],
) -> Result<ChannelAttachment> {
	upload_for_run(store, lease, thread, input, content, None).await
}
pub(crate) async fn upload_for_run(
	store: &Store,
	lease: &mut Lease,
	thread: Uuid,
	input: ChannelAttachmentUploadQuery,
	content: &[u8],
	run_id: Option<Uuid>,
) -> Result<ChannelAttachment> {
	aidash_domain::threads::channels::validate_attachment(
		&input.filename,
		&input.media_type,
		content,
	)?;
	let media_type = input.media_type.clone();
	let bytes = content.to_vec();
	let measured =
		decode(move || aidash_integrations::media::validate(&media_type, &bytes)).await?;
	let mut scope = crate::bootstrap::channel_scope(store, lease);
	scope.run_id = run_id;
	aidash_application::threads::channels::upload(
		&mut scope,
		thread,
		aidash_application::threads::channels::AttachmentUpload {
			filename: &input.filename,
			media_type: &input.media_type,
			idempotency_key: input.idempotency_key,
			measurements: aidash_application::threads::channels::MediaMeasurements {
				width: measured.width,
				height: measured.height,
				duration_ms: measured.duration_ms,
			},
			description: input.description.as_deref(),
		},
		content,
	)
	.await
	.map_err(Into::into)
}
/// Replace an image's Image Description as the person who posted it (#622).
pub(crate) async fn describe(
	store: &Store,
	lease: &mut Lease,
	thread: Uuid,
	attachment: Uuid,
	description: &str,
) -> Result<ChannelAttachment> {
	aidash_application::threads::channels::describe(
		&mut crate::bootstrap::channel_scope(store, lease),
		thread,
		attachment,
		description,
	)
	.await
	.map_err(Into::into)
}
pub(crate) async fn download(
	store: &Store,
	lease: &mut Lease,
	thread: Uuid,
	id: Uuid,
) -> Result<(ChannelAttachment, Vec<u8>)> {
	aidash_application::threads::channels::download(
		&mut crate::bootstrap::channel_scope(store, lease),
		thread,
		id,
	)
	.await
	.map_err(Into::into)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test(start_paused = true)]
	async fn slot_wait_times_out_as_decoder_busy_without_starting_work() {
		let result = decode_bounded::<()>(Arc::new(Semaphore::new(0)), DECODE_DEADLINE, || {
			panic!("no decoder slot was granted")
		})
		.await;
		let response = result.unwrap_err().http_response();
		assert_eq!(response.status.as_u16(), 503);
		let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
		assert_eq!(body["code"], "media_decoder_busy");
		assert!(body["error"].as_str().unwrap().contains("10 seconds"));
	}

	#[tokio::test]
	async fn decoder_distinguishes_worker_failures_from_invalid_media() {
		let closed = Arc::new(Semaphore::new(1));
		closed.close();
		let closed_error = decode_bounded::<()>(closed, DECODE_DEADLINE, || unreachable!())
			.await
			.unwrap_err();
		assert_eq!(closed_error.http_response().status.as_u16(), 500);

		let crash = decode::<()>(|| panic!("decoder panic")).await.unwrap_err();
		assert_eq!(crash.http_response().status.as_u16(), 500);

		let malformed =
			decode::<()>(|| Err(aidash_integrations::media::Refusal::Undecodable)).await;
		assert!(
			matches!(malformed, Err(crate::Error::Invalid(code)) if code == "MEDIA_UNDECODABLE")
		);

		let slow = decode_bounded(
			Arc::new(Semaphore::new(1)),
			Duration::from_millis(10),
			|| {
				std::thread::sleep(Duration::from_millis(100));
				Ok(())
			},
		)
		.await;
		assert!(matches!(slow, Err(crate::Error::MediaDecodeTimeout)));
	}
}
