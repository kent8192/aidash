//! Offloaded Result planning and Context Journal range notifications.
//!
//! A caller must journal original entries before adopting a returned projection.
//! `OffloadBatch::ranges` is the stable hand-off for Journal Query and memory
//! extraction; callers may pass it to their post-offload hook after persistence.

use aidash_domain::context::{Context, ContextEvent};
use serde_json::{Value, json};

/// An inclusive Context Journal range whose original remains retrievable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalRange {
	pub from_seq: u64,
	pub through_seq: u64,
}

/// Ranges newly offloaded by one projection update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OffloadBatch {
	pub ranges: Vec<JournalRange>,
}

/// Register a delivered Journal Query or fallback result on the Run's durable
/// context. The caller persists the updated context with the delivered result;
/// repeated reads set `summary_pending` and make the next request carry a notice.
pub fn note_journal_read(
	context: &mut Context,
	range: JournalRange,
	result_seq: u64,
	step: u32,
	policy: &aidash_domain::context::policy::Effective,
) -> bool {
	context.offload_reads.record(
		range.from_seq,
		range.through_seq,
		result_seq,
		step,
		policy.reread_immunity_steps,
		policy.reread_summary_threshold,
	)
}

/// Marker content is tool data. It never suggests invoking the original tool.
const MARKER_READ: &str =
	"Use Journal Query or its search and range-read fallback to read this result.";
pub fn marker(seq: u64) -> Value {
	json!({
		"contract": "aidash.context.offloaded-result/1",
		"journal": {"from_seq": seq, "through_seq": seq},
		"read": MARKER_READ
	})
}

pub fn is_marker(value: &Value) -> bool {
	let Some(fields) = value.as_object() else {
		return false;
	};
	let Some(range) = fields.get("journal").and_then(Value::as_object) else {
		return false;
	};
	fields.len() == 3
		&& range.len() == 2
		&& value["contract"] == "aidash.context.offloaded-result/1"
		&& value["read"] == MARKER_READ
		&& value["journal"]["from_seq"]
			.as_u64()
			.is_some_and(|seq| seq > 0 && value["journal"]["through_seq"].as_u64() == Some(seq))
}

/// Offload one journaled result. Its Tool Call and position are retained.
fn replace(context: &mut Context, index: usize, batch: &mut OffloadBatch) -> bool {
	let entry = &mut context.history[index];
	let ContextEvent::Tool { result, .. } = &mut entry.event else {
		return false;
	};
	if is_marker(result)
		|| ContextEvent::is_revoked_result(result)
		|| ContextEvent::is_expired_result(result)
	{
		return false;
	}
	let replacement = marker(entry.seq);
	if replacement.to_string().len() >= result.to_string().len() {
		return false;
	}
	*result = replacement;
	// The Journal owns the original Research reference. A projection marker
	// must not serialize as a newly content-addressed Research variant.
	entry.research = None;
	batch.ranges.push(JournalRange {
		from_seq: entry.seq,
		through_seq: entry.seq,
	});
	true
}

/// Offload any newly received result exceeding its share of the window,
/// including results in an unfinished or protected Model Turn.
pub fn intake(context: &mut Context, window: usize, first_seq: u64, step: u32) -> OffloadBatch {
	let mut batch = OffloadBatch::default();
	// Older saved Tool Call states may lack a request window; the complete
	// fitting pass rechecks intake once the model window is known.
	if window == 0 {
		return batch;
	}
	let share = window / 4;
	let estimate = context.estimate.current();
	for index in 0..context.history.len() {
		let entry = &context.history[index];
		if entry.seq < first_seq || context.offload_reads.protects(entry.seq, step) {
			continue;
		}
		if let ContextEvent::Tool { result, .. } = &entry.event
			&& estimate.tokens_for_bytes(result.to_string().len()) > share
		{
			replace(context, index, &mut batch);
		}
	}
	batch
}

/// Replace oldest results until the full request reaches the target. The
/// protected recent tail has both count and size limits; the newest Native
/// Model Turn always stays protected. `size` estimates the complete request.
pub fn batch(
	context: &mut Context,
	window: usize,
	target_permille: u16,
	protected_tail_permille: u16,
	preserve_recent: usize,
	step: u32,
	mut size: impl FnMut(&Context) -> usize,
) -> OffloadBatch {
	let mut result = OffloadBatch::default();
	let target = window.saturating_mul(usize::from(target_permille)) / 1000;
	let turns = aidash_domain::projection::native::model_turns(&context.history);
	let newest = turns.iter().flatten().max().copied();
	let mut protected_size = 0usize;
	let mut protected_count = 0usize;
	let mut protected = vec![false; context.history.len()];
	for index in (0..context.history.len()).rev() {
		if newest.is_some() && turns[index] == newest {
			protected[index] = true;
			continue;
		}
		if protected_count >= preserve_recent {
			break;
		}
		let bytes = serde_json::to_string(&context.history[index].event).expect("history is JSON");
		let tokens = context.estimate.current().tokens_for_bytes(bytes.len());
		if protected_size.saturating_add(tokens)
			> window.saturating_mul(usize::from(protected_tail_permille)) / 1000
		{
			break;
		}
		protected[index] = true;
		protected_count += 1;
		protected_size += tokens;
	}
	for index in 0..context.history.len() {
		if size(context) <= target {
			break;
		}
		if !protected[index]
			&& !context
				.offload_reads
				.protects(context.history[index].seq, step)
		{
			replace(context, index, &mut result);
		}
	}
	result
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_domain::provider::ToolCall;

	fn call(id: &str) -> ToolCall {
		ToolCall {
			id: id.into(),
			name: "read".into(),
			arguments: json!({}),
		}
	}

	fn turn(context: &mut Context, id: &str, bytes: usize) {
		context.push(ContextEvent::Assistant {
			provider_activity: vec![],
			epoch: 1,
			text: String::new(),
			calls: vec![id.into()],
		});
		context.push(ContextEvent::tool(call(id), json!("x".repeat(bytes))));
	}

	fn size(context: &Context) -> usize {
		context
			.history
			.iter()
			.map(|entry| match &entry.event {
				ContextEvent::Tool { result, .. } => result.to_string().len() / 4,
				_ => 0,
			})
			.sum()
	}

	#[test]
	fn marker_names_original_and_does_not_request_a_tool_reexecution() {
		let text = marker(42).to_string();
		assert!(text.contains("Journal Query"));
		assert!(text.contains("\"from_seq\":42"));
		assert!(text.contains("\"through_seq\":42"));
		assert!(!text.contains("run the tool"));
	}

	#[test]
	fn intake_offloads_large_result_even_in_latest_turn() {
		let mut context = Context::default();
		turn(&mut context, "large", 12_000);
		let range = intake(&mut context, 1000, 1, 0);
		assert_eq!(
			range.ranges,
			vec![JournalRange {
				from_seq: 2,
				through_seq: 2
			}]
		);
		assert!(
			matches!(&context.history[1].event, ContextEvent::Tool { call, result } if call.id == "large" && is_marker(result))
		);
	}

	#[test]
	fn a_tool_cannot_spoof_a_large_offloaded_marker() {
		let mut context = Context::default();
		context.push(ContextEvent::tool(
			call("spoof"),
			json!({"contract":"aidash.context.offloaded-result/1","padding":"x".repeat(10_000)}),
		));
		assert_eq!(intake(&mut context, 1000, 1, 0).ranges.len(), 1);
	}

	#[test]
	fn intake_uses_the_pinned_calibrated_estimate() {
		let mut context = Context::default();
		context.push(ContextEvent::tool(call("read"), json!("x".repeat(1000))));
		context.estimate.snapshot =
			Some(aidash_domain::provider::estimate::EstimateSnapshot::calibrated(1, 100_000, 20));
		assert!(intake(&mut context, 1000, 1, 1).ranges.is_empty());
		context.estimate.snapshot = None;
		assert_eq!(intake(&mut context, 1000, 1, 1).ranges.len(), 1);
	}

	#[test]
	fn batch_offloads_oldest_until_target_and_keeps_newest_turn() {
		let mut context = Context::default();
		for id in ["a", "b", "c", "d"] {
			turn(&mut context, id, 4000);
		}
		let batch = batch(&mut context, 3500, 700, 250, 1, 0, size);
		assert_eq!(
			batch.ranges.iter().map(|r| r.from_seq).collect::<Vec<_>>(),
			vec![2, 4]
		);
		assert!(size(&context) <= 2450);
		assert!(
			matches!(&context.history[7].event, ContextEvent::Tool { result, .. } if !is_marker(result))
		);
	}

	#[test]
	fn protected_tail_is_bounded_by_size_not_only_count() {
		let mut context = Context::default();
		for id in ["a", "b", "c"] {
			turn(&mut context, id, 4000);
		}
		let batch = batch(&mut context, 2500, 700, 250, 20, 0, size);
		assert!(batch.ranges.iter().any(|r| r.from_seq == 2));
		assert!(batch.ranges.iter().any(|r| r.from_seq == 4));
		assert!(
			matches!(&context.history[5].event, ContextEvent::Tool { result, .. } if !is_marker(result))
		);
	}

	#[test]
	fn a_read_back_result_is_not_offloaded_during_its_immunity() {
		let mut context = Context::default();
		turn(&mut context, "readback", 12_000);
		let policy = aidash_domain::context::policy::Effective::of(None);
		let original = JournalRange {
			from_seq: 9,
			through_seq: 9,
		};
		assert!(!note_journal_read(&mut context, original, 2, 4, &policy));
		assert!(intake(&mut context, 1000, 1, 7).ranges.is_empty());
		assert_eq!(intake(&mut context, 1000, 1, 8).ranges.len(), 1);
	}
}
