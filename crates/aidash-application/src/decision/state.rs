//! Shared bounded state utilities for decision hooks.
use super::*;

const REQUEST_ENVELOPE_BYTES: usize = 256;

/// The encoded size of the largest request a stage needs: its state with the
/// one question whose encoding is largest. Larger batches split per question.
pub(super) fn single_question_request_bytes(state: &Value, questions: &Questions) -> Result<usize> {
	let state_bytes = serde_json::to_vec(state)?.len();
	// Count the fixed object framing once, replacing null and {} with the
	// actual state and one encoded question. State is never cloned per question.
	let framing = serde_json::to_vec(&json!({"state": null, "questions": {}}))?.len() - 6;
	let mut largest = 0;
	for (id, question) in questions {
		let one = json!({id: {"type": "noul", "instructions": question.description}});
		largest = largest.max(framing + state_bytes + serde_json::to_vec(&one)?.len());
	}
	Ok(largest + REQUEST_ENVELOPE_BYTES)
}

pub(super) fn abridge(input: &Value, limit: Option<usize>) -> Value {
	let Some(limit) = limit else {
		return input.clone();
	};
	let text = input.to_string();
	if text.chars().count() <= limit {
		return input.clone();
	}
	let head: String = text.chars().take(limit.saturating_sub(1)).collect();
	json!(format!("{head}…"))
}
