use super::*;

fn text(value: &str) -> InferenceProgress {
	InferenceProgress::Text { text: value.into() }
}

#[test]
fn text_coalesces_only_within_the_item_bound() {
	let mut item = text("hello ");
	assert!(item.absorb(&text("world")));
	assert_eq!(item, text("hello world"));
	let large = "x".repeat(MAX_PROGRESS_ITEM_BYTES / 2);
	assert!(!item.absorb(&text(&large)));
	assert_eq!(item, text("hello world"));
}

#[test]
fn tool_call_status_merges_per_index_without_arguments() {
	let mut item = InferenceProgress::ToolCall {
		index: 0,
		id: Some("call_1".into()),
		name: None,
		argument_bytes: 3,
	};
	assert!(item.absorb(&InferenceProgress::ToolCall {
		index: 0,
		id: None,
		name: Some("read".into()),
		argument_bytes: 9,
	}));
	assert_eq!(
		item,
		InferenceProgress::ToolCall {
			index: 0,
			id: Some("call_1".into()),
			name: Some("read".into()),
			argument_bytes: 9,
		}
	);
	assert!(!item.absorb(&InferenceProgress::ToolCall {
		index: 1,
		id: None,
		name: None,
		argument_bytes: 1,
	}));
	assert!(!item.absorb(&text("tail")));
}

#[test]
fn outcomes_name_their_marker_and_reason() {
	assert_eq!(ProgressOutcome::Accepted.event_kind(), "inference.accepted");
	assert_eq!(ProgressOutcome::Discarded.reason(), "correction");
	let lost = ProgressOutcome::Interrupted(InterruptionReason::LeaseLost);
	assert_eq!(lost.event_kind(), "inference.interrupted");
	assert_eq!(
		serde_json::to_value(lost).unwrap(),
		serde_json::json!({"outcome":"interrupted","reason":"lease_lost"})
	);
}
