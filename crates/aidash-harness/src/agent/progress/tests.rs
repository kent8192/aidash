//! Stored progress items stay within the per-item bound for any text.
use super::*;
use rstest::rstest;

#[rstest]
#[case::plain("a")]
#[case::multibyte("界")]
#[case::escaped("\"")]
#[case::control("\u{1}")]
fn split_text_keeps_every_serialized_item_within_the_bound(#[case] unit: &str) {
	// Arrange
	let text = unit.repeat(3 * MAX_PROGRESS_ITEM_BYTES / unit.len());
	// Act
	let batch = bounded(vec![InferenceProgress::Text { text: text.clone() }]);
	// Assert
	assert!(batch.len() > 1);
	assert!(batch.iter().all(|item| {
		serde_json::to_vec(item).unwrap().len() + STORAGE_SLACK <= MAX_PROGRESS_ITEM_BYTES
	}));
	let joined: String = batch
		.iter()
		.map(|item| match item {
			InferenceProgress::Text { text } => text.as_str(),
			InferenceProgress::ToolCall { .. } => unreachable!("only text was offered"),
		})
		.collect();
	assert_eq!(joined, text);
}

#[rstest]
fn offers_merge_until_the_pending_bound_then_drop() {
	// Arrange
	let buffer = ProgressBuffer::new(Instant::now());
	let chunk = "x".repeat(1024);
	// Act
	for _ in 0..2 * MAX_PENDING_BYTES / chunk.len() {
		buffer.offer(InferenceProgress::Text {
			text: chunk.clone(),
		});
	}
	// Assert
	let pending = buffer.pending();
	assert_eq!(pending.bytes, MAX_PENDING_BYTES);
	assert_eq!(
		pending
			.items
			.iter()
			.map(InferenceProgress::weight)
			.sum::<usize>(),
		MAX_PENDING_BYTES
	);
	assert!(pending.items.len() < MAX_PENDING_BYTES / chunk.len());
}
