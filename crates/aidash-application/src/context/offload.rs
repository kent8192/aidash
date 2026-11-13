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

/// Track a delivered Journal tool result. Only a pinned Journal builtin or
/// descriptor may register a read; model aliases and other tools cannot spoof it.
#[allow(clippy::too_many_arguments)] // The read is recorded with the call, its result position and the policy it ran under.
pub fn note_journal_result(
	context: &mut Context,
	reader_run: uuid::Uuid,
	contract: &aidash_domain::tool::ToolContract,
	input: &Value,
	result: &Value,
	result_seq: u64,
	step: u32,
	policy: &aidash_domain::context::policy::Effective,
) {
	use aidash_domain::tool::{ToolIdentity, providers::ToolDescriptor};
	let operation = match &contract.identity {
		ToolIdentity::Builtin(id) => id.strip_prefix("builtin:").map(str::to_owned),
		ToolIdentity::Descriptor(id) => context.binding_snapshot.as_ref().and_then(|snapshot| {
			snapshot
				.bindings
				.iter()
				.find(|binding| &binding.identity == id)
				.and_then(|binding| {
					serde_json::from_value::<ToolDescriptor>(binding.definition.config.clone()).ok()
				})
				.filter(|descriptor| descriptor.provider == "core.journal@1")
				.map(|descriptor| descriptor.operation)
		}),
		_ => None,
	};
	let range = match operation.as_deref() {
		Some("journal_query")
			if result["contract"] == "aidash.journal.query/1"
				&& result["stdout"].is_string()
				&& (result.get("error").is_none()
					|| result["stdout"].as_str().is_some_and(|s| !s.is_empty())) =>
		{
			input.get("journal_range").unwrap_or(&Value::Null)
		}
		Some("journal_read") if result["text"].is_string() => input,
		Some("journal_search") if result["results"].is_array() => input,
		_ => return,
	};
	let target_run = match input.get("run_id") {
		None | Some(Value::Null) => None,
		Some(value) => match value.as_str().and_then(|id| uuid::Uuid::parse_str(id).ok()) {
			Some(id) if id == reader_run => None,
			Some(id) => Some(id),
			None => return,
		},
	};
	// A read without a range, such as a search or query of the whole
	// Journal, only protects its result: it never counts as reading one range
	// again (Decision 7).
	if range.get("from_seq").is_none() && range.get("through_seq").is_none() {
		context
			.offload_reads
			.protect(target_run, result_seq, step, policy.reread_immunity_steps);
		return;
	}
	let from_seq = range
		.get("from_seq")
		.and_then(Value::as_u64)
		.unwrap_or(0)
		.max(1);
	let through_seq = range
		.get("through_seq")
		.and_then(Value::as_u64)
		.unwrap_or(i64::MAX as u64);
	// Paging through one range delivers new text: only a page already
	// delivered counts as reading it again. A search pages by its cursor.
	let offset = match operation.as_deref() {
		Some("journal_read") => result.get("offset").and_then(Value::as_u64),
		Some("journal_search") => input
			.get("cursor")
			.and_then(Value::as_str)
			.and_then(|cursor| cursor.parse().ok()),
		_ => None,
	}
	.unwrap_or(0);
	if from_seq <= through_seq && through_seq <= i64::MAX as u64 {
		context.offload_reads.record_for(
			target_run,
			aidash_domain::context::offload::Page {
				from_seq,
				through_seq,
				offset,
			},
			result_seq,
			step,
			policy.reread_immunity_steps,
			policy.reread_summary_threshold,
		);
	}
}

/// Marker content is tool data. It never suggests invoking the original tool.
const MARKER_READ: &str =
	"Use Journal Query or its search and range-read fallback to read this result.";
pub fn marker(seq: u64, original: &Value) -> Value {
	let text = original
		.as_str()
		.map(str::to_owned)
		.unwrap_or_else(|| original.to_string());
	let head: String = text.chars().take(300).collect();
	json!({
		"contract": "aidash.context.offloaded-result/1",
		"journal": {"from_seq": seq, "through_seq": seq},
		"read": MARKER_READ,
		"head": head
	})
}

pub fn is_marker(value: &Value) -> bool {
	let Some(fields) = value.as_object() else {
		return false;
	};
	let Some(range) = fields.get("journal").and_then(Value::as_object) else {
		return false;
	};
	fields.len() == 4
		&& range.len() == 2
		&& value["contract"] == "aidash.context.offloaded-result/1"
		&& value["read"] == MARKER_READ
		&& value["head"]
			.as_str()
			.is_some_and(|head| head.chars().count() <= 300)
		&& value["journal"]["from_seq"]
			.as_u64()
			.is_some_and(|seq| seq > 0 && value["journal"]["through_seq"].as_u64() == Some(seq))
}

/// Offload one journaled result. Its Tool Call and position are retained.
/// Native evidence offloads item by item and keeps each item's digest, so a
/// later step never appends it again (#942). A step state other than the
/// latest one, which says what holds now, offloads whole.
fn replace(context: &mut Context, index: usize, batch: &mut OffloadBatch) -> bool {
	let latest_state = context
		.history
		.iter()
		.rposition(|entry| matches!(entry.event, ContextEvent::StepState { .. }));
	let entry = &mut context.history[index];
	let seq = entry.seq;
	let mut shrink = |value: &mut Value| {
		if is_marker(value) {
			return false;
		}
		let replacement = marker(seq, value);
		if replacement.to_string().len() >= value.to_string().len() {
			return false;
		}
		*value = replacement;
		true
	};
	let replaced = match &mut entry.event {
		ContextEvent::Evidence { items } => items
			.iter_mut()
			.fold(false, |any, item| shrink(&mut item.content) || any),
		ContextEvent::StepState { state, harness } if latest_state != Some(index) => {
			let shrunk = shrink(state);
			shrunk | harness.as_mut().is_some_and(&mut shrink)
		}
		_ => false,
	};
	if !matches!(entry.event, ContextEvent::Tool { .. }) {
		if replaced {
			context.offloaded.get_or_insert_default();
			batch.ranges.push(JournalRange {
				from_seq: seq,
				through_seq: seq,
			});
		}
		return replaced;
	}
	let ContextEvent::Tool { result, .. } = &mut entry.event else {
		return false;
	};
	if is_marker(result)
		|| ContextEvent::is_revoked_result(result)
		|| ContextEvent::is_expired_result(result)
	{
		return false;
	}
	let replacement = marker(entry.seq, result);
	if replacement.to_string().len() >= result.to_string().len() {
		return false;
	}
	*result = replacement;
	// The Journal owns the original Research reference. A projection marker
	// must not serialize as a newly content-addressed Research variant.
	entry.research = None;
	context.offloaded.get_or_insert_default();
	batch.ranges.push(JournalRange {
		from_seq: entry.seq,
		through_seq: entry.seq,
	});
	true
}

/// Offload newly received results exceeding their share of the window,
/// including results in an unfinished or protected Model Turn. The results of
/// one Model Turn, such as parallel calls (#613), share one share: its largest
/// results are offloaded until the rest fit in it. A result outside a turn
/// has the share to itself.
pub fn intake(context: &mut Context, window: usize, first_seq: u64, step: u32) -> OffloadBatch {
	let mut batch = OffloadBatch::default();
	// Older saved Tool Call states may lack a request window; the complete
	// fitting pass rechecks intake once the model window is known.
	if window == 0 {
		return batch;
	}
	let share = window / 4;
	let estimate = context.estimate.current();
	let turns = aidash_domain::projection::native::model_turns(&context.history);
	let mut groups =
		std::collections::BTreeMap::<(Option<usize>, usize), Vec<(usize, usize)>>::new();
	for (index, entry) in context.history.iter().enumerate() {
		if entry.seq < first_seq || context.offload_reads.protects(entry.seq, step) {
			continue;
		}
		if let ContextEvent::Tool { result, .. } = &entry.event
			&& !is_marker(result)
		{
			let key = match turns[index] {
				Some(turn) => (Some(turn), 0),
				None => (None, index),
			};
			let tokens = estimate.tokens_for_bytes(result.to_string().len());
			groups.entry(key).or_default().push((tokens, index));
		}
	}
	for mut results in groups.into_values() {
		let mut total: usize = results.iter().map(|(tokens, _)| *tokens).sum();
		results.sort_by(|a, b| b.cmp(a));
		for (tokens, index) in results {
			if total <= share {
				break;
			}
			if replace(context, index, &mut batch) {
				total = total.saturating_sub(tokens);
			}
		}
	}
	batch.ranges.sort_by_key(|range| range.from_seq);
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
	// `preserve_recent` counts Model Turns, as the policy and the Summary
	// Stage do; an event outside a turn counts alone.
	let mut protected_count = 0usize;
	let mut current = None;
	let mut protected = vec![false; context.history.len()];
	for index in (0..context.history.len()).rev() {
		if newest.is_some() && turns[index] == newest {
			protected[index] = true;
			continue;
		}
		let same_turn = turns[index].is_some() && turns[index] == current;
		// Native step states and evidence sit between turns; they are
		// protected with the tail around them but take no turn's place.
		let between_turns = matches!(
			context.history[index].event,
			ContextEvent::StepState { .. } | ContextEvent::Evidence { .. }
		);
		if !same_turn && !between_turns && protected_count >= preserve_recent {
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
		if !same_turn && !between_turns {
			protected_count += 1;
			current = turns[index];
		}
		protected_size += tokens;
	}
	for (index, kept) in protected.into_iter().enumerate() {
		if size(context) <= target {
			break;
		}
		if !kept
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

	/// Record a Native step's state and evidence, as the Harness does.
	fn step(context: &mut Context, phase: &str, tasks: &[String]) {
		let mut pinned = json!({
			"agent_state": {"phase": phase},
			"turn_instructions": "i".repeat(1000),
			"workspace": {"tasks": tasks},
		});
		aidash_domain::projection::native::record_step_state(context, &pinned);
		aidash_domain::projection::native::record_evidence(context, &mut pinned, usize::MAX);
	}

	fn whole(context: &Context) -> usize {
		context
			.history
			.iter()
			.map(|entry| serde_json::to_string(&entry.event).unwrap().len() / 4)
			.sum()
	}

	#[test]
	fn a_compaction_step_keeps_the_latest_state_and_never_reappends_evidence() {
		// Arrange: three Native steps, each with a new state and new evidence.
		let mut context = Context::default();
		let task = |n: usize| format!("{n}{}", "t".repeat(2000));
		let mut tasks = Vec::new();
		for (n, phase) in ["a", "b", "c"].into_iter().enumerate() {
			tasks.push(task(n));
			step(&mut context, phase, &tasks);
			turn(&mut context, &format!("call-{n}"), 10);
		}
		let latest = context
			.history
			.iter()
			.rposition(|entry| matches!(entry.event, ContextEvent::StepState { .. }))
			.unwrap();
		let latest_state = context.history[latest].clone();
		// Act
		let batch = batch(&mut context, 1000, 100, 1, 1, 0, whole);
		// Assert: older evidence and states are markers, the latest state is
		// whole and the newest turn's evidence stays protected.
		assert!(!batch.ranges.is_empty());
		assert_eq!(context.history[latest], latest_state);
		let first_evidence = context
			.history
			.iter()
			.find_map(|entry| match &entry.event {
				ContextEvent::Evidence { items } => Some(items),
				_ => None,
			})
			.unwrap();
		assert!(is_marker(&first_evidence[0].content));
		assert!(matches!(
			&context.history[0].event,
			ContextEvent::StepState { state, .. } if is_marker(state)
		));
		// The next step appends neither offloaded evidence nor an unchanged state.
		let before = context.history.len();
		step(&mut context, "c", &tasks);
		assert_eq!(context.history.len(), before);
		let pinned = json!({"workspace": {"tasks": tasks}});
		let delivered =
			aidash_domain::projection::native::delivered_evidence(&context, &pinned, "workspace")
				.unwrap();
		assert_eq!(delivered["tasks"][0], Value::Null);
	}

	#[test]
	fn marker_names_original_and_does_not_request_a_tool_reexecution() {
		let original = json!(format!("{}tail", "あ".repeat(300)));
		let value = marker(42, &original);
		assert_eq!(value["head"], "あ".repeat(300));
		assert!(is_marker(&value));
		let text = value.to_string();
		assert!(text.contains("Journal Query"));
		assert!(text.contains("\"from_seq\":42"));
		assert!(text.contains("\"through_seq\":42"));
		assert!(!text.contains("run the tool"));
	}

	#[test]
	fn intake_offloads_large_result_even_in_latest_turn() {
		let mut context = Context::default();
		assert!(!aidash_domain::context::journal_query::offloaded_any(
			&context
		));
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
		let restored: Context =
			serde_json::from_value(serde_json::to_value(context).unwrap()).unwrap();
		assert!(aidash_domain::context::journal_query::offloaded_any(
			&restored
		));
	}

	#[test]
	fn completed_journal_results_share_immunity_and_escalation() {
		let mut context = Context::default();
		let policy = aidash_domain::context::policy::Effective::of(None);
		let input = json!({"from_seq":3,"through_seq":5});
		for (name, arguments, result) in [
			(
				"journal_read",
				input.clone(),
				json!({"text":"x".repeat(10_000)}),
			),
			(
				"journal_search",
				input.clone(),
				json!({"results":[{"seq":3}]}),
			),
			(
				"journal_query",
				json!({"journal_range":input}),
				json!({"contract":"aidash.journal.query/1","stdout":"found"}),
			),
		] {
			context.push(ContextEvent::tool(call(name), result.clone()));
			let seq = context.journal.head;
			let contract = aidash_domain::tool::builtin_contract(name).unwrap();
			note_journal_result(
				&mut context,
				uuid::Uuid::nil(),
				&contract,
				&arguments,
				&result,
				seq,
				10,
				&policy,
			);
		}
		assert_eq!(context.offload_reads.ranges.len(), 3);
		assert!(context.offload_reads.summary_pending && context.offload_reads.notice_pending);
		assert!(intake(&mut context, 1000, 1, 13).ranges.is_empty());
		assert_eq!(intake(&mut context, 1000, 1, 14).ranges.len(), 1);
		let restored: Context =
			serde_json::from_value(serde_json::to_value(context).unwrap()).unwrap();
		assert_eq!(restored.offload_reads.ranges[2].reads, 3);
	}

	#[test]
	fn reads_of_the_same_sequence_in_different_runs_are_counted_separately() {
		let mut context = Context::default();
		let policy = aidash_domain::context::policy::Effective::of(None);
		let contract = aidash_domain::tool::builtin_contract("journal_read").unwrap();
		let creator = uuid::Uuid::from_u128(42);
		let reader = uuid::Uuid::from_u128(1);
		for (seq, input) in [
			(1, json!({"from_seq":3,"through_seq":5})),
			(2, json!({"run_id":creator,"from_seq":3,"through_seq":5})),
			(3, json!({"run_id":creator,"from_seq":3,"through_seq":5})),
			(4, json!({"run_id":reader,"from_seq":3,"through_seq":5})),
		] {
			note_journal_result(
				&mut context,
				reader,
				&contract,
				&input,
				&json!({"text":"result"}),
				seq,
				10,
				&policy,
			);
		}
		assert_eq!(
			context
				.offload_reads
				.ranges
				.iter()
				.map(|read| read.reads)
				.collect::<Vec<_>>(),
			[1, 1, 2, 2]
		);
		assert!(!context.offload_reads.summary_pending);
		let mut restored: Context =
			serde_json::from_value(serde_json::to_value(context).unwrap()).unwrap();
		assert_eq!(restored.offload_reads.ranges[1].target_run, Some(creator));
		let query = aidash_domain::tool::builtin_contract("journal_query").unwrap();
		note_journal_result(
			&mut restored,
			reader,
			&query,
			&json!({"run_id":creator,"journal_range":{"from_seq":3,"through_seq":5}}),
			&json!({"contract":"aidash.journal.query/1","stdout":"result"}),
			5,
			10,
			&policy,
		);
		assert_eq!(restored.offload_reads.ranges[4].reads, 3);
		assert!(restored.offload_reads.summary_pending);
	}

	#[test]
	fn paging_and_unranged_reads_do_not_ask_for_a_summary() {
		let mut context = Context::default();
		let policy = aidash_domain::context::policy::Effective::of(None);
		let read = aidash_domain::tool::builtin_contract("journal_read").unwrap();
		let search = aidash_domain::tool::builtin_contract("journal_search").unwrap();
		let query = aidash_domain::tool::builtin_contract("journal_query").unwrap();
		let mut seq = 0;
		let mut note = |context: &mut Context,
		                contract: &aidash_domain::tool::ToolContract,
		                input: Value,
		                result: Value| {
			seq += 1;
			note_journal_result(
				context,
				uuid::Uuid::nil(),
				contract,
				&input,
				&result,
				seq,
				10,
				&policy,
			);
		};
		// One 9000-char original read in three pages.
		for offset in [0, 4000, 8000] {
			note(
				&mut context,
				&read,
				json!({"from_seq":3,"through_seq":3,"offset":offset}),
				json!({"text":"x","offset":offset,"total_chars":9000}),
			);
		}
		for _ in 0..3 {
			note(
				&mut context,
				&search,
				json!({"query":"needle"}),
				json!({"results":[{"seq":3}]}),
			);
			note(
				&mut context,
				&query,
				json!({"code":"journal()"}),
				json!({"contract":"aidash.journal.query/1","stdout":"ok"}),
			);
		}
		assert!(!context.offload_reads.summary_pending);
		assert!(!context.offload_reads.notice_pending);
		// Reading the first page again still counts.
		for _ in 0..2 {
			note(
				&mut context,
				&read,
				json!({"from_seq":3,"through_seq":3}),
				json!({"text":"x","offset":0,"total_chars":9000}),
			);
		}
		assert!(context.offload_reads.summary_pending);
	}

	#[test]
	fn failed_or_spoofed_journal_results_do_not_record_reads() {
		let mut context = Context::default();
		let policy = aidash_domain::context::policy::Effective::of(None);
		let input = json!({"from_seq":1,"through_seq":2});
		for (name, result) in [
			("journal_read", json!({"error":"refused"})),
			("journal_query", json!({"session_reset":true})),
			("file_read", json!({"text":"spoofed"})),
		] {
			let contract = aidash_domain::tool::builtin_contract(name).unwrap();
			note_journal_result(
				&mut context,
				uuid::Uuid::nil(),
				&contract,
				&input,
				&result,
				1,
				0,
				&policy,
			);
		}
		assert!(context.offload_reads.is_empty());
		let contract = aidash_domain::tool::builtin_contract("journal_query").unwrap();
		note_journal_result(
			&mut context,
			uuid::Uuid::nil(),
			&contract,
			&json!({"code":"journal()"}),
			&json!({"contract":"aidash.journal.query/1","stdout":"ok"}),
			1,
			0,
			&policy,
		);
		assert_eq!(
			(
				context.offload_reads.ranges[0].from_seq,
				context.offload_reads.ranges[0].through_seq
			),
			(1, i64::MAX as u64)
		);
	}

	#[test]
	fn parallel_results_of_one_turn_share_the_intake_share() {
		// #613: eight results of one Model Turn, each within the share alone,
		// together larger than the window.
		let mut context = Context::default();
		let ids: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
		context.push(ContextEvent::Assistant {
			provider_activity: vec![],
			epoch: 1,
			text: String::new(),
			calls: ids.clone(),
		});
		for id in &ids {
			context.push(ContextEvent::tool(call(id), json!("x".repeat(4000))));
		}
		let estimate = context.estimate.current();
		let tokens = |context: &Context| -> Vec<usize> {
			context
				.history
				.iter()
				.filter_map(|entry| match &entry.event {
					ContextEvent::Tool { result, .. } if !is_marker(result) => {
						Some(estimate.tokens_for_bytes(result.to_string().len()))
					}
					_ => None,
				})
				.collect()
		};
		let one = tokens(&context)[0];
		let window = one * 8;
		assert!(one <= window / 4, "each result fits the share alone");
		let batch = intake(&mut context, window, 1, 0);
		assert!(!batch.ranges.is_empty());
		// The originals left in the projection fit the turn's share.
		assert!(tokens(&context).iter().sum::<usize>() <= window / 4);
		assert!(
			tokens(&context).len() >= 2,
			"only what exceeds the share goes"
		);
	}

	#[test]
	fn a_tool_cannot_spoof_a_large_offloaded_marker() {
		let mut spoof = marker(1, &json!("small"));
		spoof["head"] = json!("x".repeat(301));
		assert!(!is_marker(&spoof));
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
