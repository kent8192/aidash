//! Display-only Inference Progress of one attempt. Offers never block or fail
//! the provider call; stored batches stay within the per-item bound.
use aidash_application::ports::{InferenceProgressSink, execution::ExecutionStore};
use aidash_domain::{
	Run,
	provider::progress::{
		FLUSH_BYTES, FLUSH_INTERVAL, InferenceAttemptId, InferenceProgress, MAX_PENDING_BYTES,
		MAX_PROGRESS_ITEM_BYTES,
	},
};
use std::sync::{Mutex, MutexGuard, PoisonError};
use tokio::{
	sync::Notify,
	time::{Instant, MissedTickBehavior},
};
use uuid::Uuid;

/// Room for the storage encoding's formatting around one serialized item.
const STORAGE_SLACK: usize = 64;
/// Escaped text bytes per stored item, leaving room for the item's JSON envelope.
const TEXT_BUDGET: usize = MAX_PROGRESS_ITEM_BYTES - 2 * STORAGE_SLACK;

/// Coalesces offered progress until the flusher stores it.
pub(super) struct ProgressBuffer {
	started: Instant,
	pending: Mutex<Pending>,
	wake: Notify,
}

#[derive(Default)]
struct Pending {
	items: Vec<InferenceProgress>,
	bytes: usize,
	offered: bool,
	finished: bool,
	closed: bool,
}

impl ProgressBuffer {
	/// `started` is when the provider call began; first-delta latency counts from it.
	pub(super) fn new(started: Instant) -> Self {
		Self {
			started,
			pending: Mutex::new(Pending::default()),
			wake: Notify::new(),
		}
	}

	fn pending(&self) -> MutexGuard<'_, Pending> {
		self.pending.lock().unwrap_or_else(PoisonError::into_inner)
	}

	/// The provider call returned; the flusher stores the remainder and stops.
	pub(super) fn finish(&self) {
		self.pending().finished = true;
		self.wake.notify_one();
	}

	/// Store pending progress every `FLUSH_INTERVAL`, or sooner once
	/// `FLUSH_BYTES` are pending, until the remainder after `finish` is stored.
	/// A failed append ends progress for this attempt; inference continues.
	pub(super) async fn flush(
		&self,
		store: &dyn ExecutionStore,
		run: &Run,
		token: Uuid,
		attempt: InferenceAttemptId,
	) {
		let mut interval = tokio::time::interval(FLUSH_INTERVAL);
		interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
		loop {
			tokio::select! {
				_ = interval.tick() => {}
				() = self.wake.notified() => {}
			}
			let (items, finished) = {
				let mut pending = self.pending();
				pending.bytes = 0;
				(std::mem::take(&mut pending.items), pending.finished)
			};
			let batch = bounded(items);
			if !batch.is_empty()
				&& let Err(error) = store
					.append_inference_progress(run, token, attempt, &batch)
					.await
			{
				tracing::warn!(run_id = %run.id, %attempt, %error, "inference progress append failed; progress display stops for this attempt");
				let mut pending = self.pending();
				pending.closed = true;
				pending.items = Vec::new();
				pending.bytes = 0;
				return;
			}
			if finished {
				return;
			}
		}
	}
}

impl InferenceProgressSink for ProgressBuffer {
	fn offer(&self, progress: InferenceProgress) {
		let mut pending = self.pending();
		if pending.closed {
			return;
		}
		if !std::mem::replace(&mut pending.offered, true) {
			metrics::histogram!("aidash_inference_first_delta_seconds")
				.record(self.started.elapsed().as_secs_f64());
		}
		let weight = progress.weight();
		if pending.bytes.saturating_add(weight) > MAX_PENDING_BYTES {
			metrics::counter!("aidash_inference_progress_coalesced_total").increment(1);
			return;
		}
		let Pending { items, bytes, .. } = &mut *pending;
		let mut merged = false;
		if let Some(last) = items.last_mut() {
			let before = last.weight();
			if last.absorb(&progress) {
				*bytes = *bytes - before + last.weight();
				merged = true;
			}
		}
		if !merged {
			items.push(progress);
			*bytes += weight;
		}
		if *bytes >= FLUSH_BYTES {
			self.wake.notify_one();
		}
	}
}

/// Split text so each stored item serializes within `MAX_PROGRESS_ITEM_BYTES`;
/// status that cannot fit is dropped and counted like other lost progress.
fn bounded(items: Vec<InferenceProgress>) -> Vec<InferenceProgress> {
	let mut batch = Vec::with_capacity(items.len());
	for item in items {
		match item {
			InferenceProgress::Text { text } => split_text(text, &mut batch),
			item => {
				if serde_json::to_vec(&item)
					.is_ok_and(|encoded| encoded.len() + STORAGE_SLACK <= MAX_PROGRESS_ITEM_BYTES)
				{
					batch.push(item);
				} else {
					metrics::counter!("aidash_inference_progress_coalesced_total").increment(1);
				}
			}
		}
	}
	batch
}

fn split_text(text: String, batch: &mut Vec<InferenceProgress>) {
	if text.is_empty() {
		return;
	}
	let mut cuts = Vec::new();
	let mut size = 0;
	for (index, character) in text.char_indices() {
		let width = escaped_len(character);
		if size + width > TEXT_BUDGET {
			cuts.push(index);
			size = 0;
		}
		size += width;
	}
	if cuts.is_empty() {
		batch.push(InferenceProgress::Text { text });
		return;
	}
	let mut start = 0;
	for cut in cuts.into_iter().chain(std::iter::once(text.len())) {
		batch.push(InferenceProgress::Text {
			text: text[start..cut].to_owned(),
		});
		start = cut;
	}
}

/// Bytes of `character` inside a JSON string.
fn escaped_len(character: char) -> usize {
	match character {
		'"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
		'\0'..='\u{1f}' => 6,
		character => character.len_utf8(),
	}
}

#[cfg(test)]
mod tests;
