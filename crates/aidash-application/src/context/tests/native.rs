//! Native Runs retain and summarize whole Model Turns (ADR 0015); the
//! decision gate's tests cover how they are pruned.
use super::*;
use aidash_domain::projection::{
	CacheScope,
	native::{NativeMessage, check_model_turns, native_context},
};

fn call(id: &str) -> provider::ToolCall {
	provider::ToolCall {
		id: id.into(),
		name: "read".into(),
		arguments: json!({"path": id}),
	}
}

/// `turns` Model Turns of two calls each with large results. Even turns
/// carry text; odd ones only call tools. Everything reached an inference.
fn turns(turns: usize) -> Context {
	let mut context = Context::default();
	context.push(ContextEvent::Human {
		request: "Never deploy on Fridays".into(),
		request_kind: "INFORMATION_REQUEST".into(),
		response: json!("acknowledged"),
	});
	for turn in 0..turns {
		let ids = [format!("t{turn}-a"), format!("t{turn}-b")];
		context.push(ContextEvent::Assistant {
			provider_activity: vec![],
			epoch: turn as i64,
			text: if turn % 2 == 0 {
				format!("Step {turn}")
			} else {
				String::new()
			},
			calls: ids.to_vec(),
			reasoning: None,
		});
		for id in &ids {
			context.push(ContextEvent::tool(call(id), json!("r".repeat(600))));
		}
	}
	context.journal.inferred_through = context.journal.head;
	context
}

/// The call IDs of every assistant message the projection renders, which
/// must also hold the Model Turn invariant.
fn rendered_turns(context: &Context) -> Vec<Vec<String>> {
	let native = native_context(context, &json!({}));
	check_model_turns(&native.history).unwrap();
	native
		.history
		.iter()
		.filter_map(|message| match message {
			NativeMessage::Assistant { tool_calls, .. } => {
				Some(tool_calls.iter().map(|call| call.id.clone()).collect())
			}
			_ => None,
		})
		.collect()
}

fn turn_ids(turn: usize) -> Vec<String> {
	vec![format!("t{turn}-a"), format!("t{turn}-b")]
}

static NATIVE: std::sync::LazyLock<RequestProjection> = std::sync::LazyLock::new(|| {
	RequestProjection::Native(CacheScope {
		tenant: "tenant-a".into(),
		key_version: 1,
	})
});

fn budget(window: usize) -> RequestBudget<'static> {
	RequestBudget {
		input_format: Default::default(),
		window,
		instructions: "Work on the task",
		tools: &[],
		max_output_tokens: 256,
		projection: &NATIVE,
		estimate: Default::default(),
	}
}

#[rstest::rstest]
fn native_summary_candidates_are_whole_turns_outside_the_protected_tail() {
	// Arrange
	let context = turns(10);
	// Act
	let candidates = native_summary_candidates(&context, 6);
	// Assert: four whole turns of three entries each, never the Human answer.
	assert_eq!(
		candidates.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
		(2..=13).collect::<Vec<_>>()
	);
	// A turn that did not fully reach an inference stops the set before it.
	let mut partial = turns(10);
	partial.journal.inferred_through = 9; // inside the third turn (seq 8..=10)
	assert_eq!(
		native_summary_candidates(&partial, 6)
			.iter()
			.map(|entry| entry.seq)
			.collect::<Vec<_>>(),
		(2..=7).collect::<Vec<_>>()
	);
}

#[test]
fn a_large_recent_tail_cannot_protect_all_older_turns() {
	let context = turns(4);
	let count = super::super::bounded_native_tail(&context, 6, 1);
	assert_eq!(count, 1);
	let candidates = native_summary_candidates(&context, count);
	assert_eq!(candidates.len(), 9);
	assert!(
		candidates
			.iter()
			.all(|entry| entry.seq < context.journal.head - 2)
	);
}

#[rstest::rstest]
fn a_turn_that_renders_nothing_takes_no_protected_slot() {
	// Arrange: the latest response's only call was deferred before it ran.
	let mut context = turns(10);
	context.push(ContextEvent::Assistant {
		provider_activity: vec![],
		epoch: 10,
		text: String::new(),
		calls: vec!["deferred".into()],
		reasoning: None,
	});
	context.journal.inferred_through = context.journal.head;
	// Act
	let candidates = native_summary_candidates(&context, 6);
	// Assert: the six latest rendered turns stay protected, as without it.
	assert_eq!(
		candidates.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
		(2..=13).collect::<Vec<_>>()
	);
}

#[rstest::rstest]
#[tokio::test]
async fn native_summaries_absorb_whole_turns_and_keep_the_recent_ones() {
	// Arrange: pruning keeps everything, so only the Summary Stage can fit.
	let context = turns(12);
	let pinned = json!({"task": {"description": "Ship the release"}});
	let mut retained = context.clone();
	let candidates = native_summary_candidates(&context, 6);
	retained.history.retain(|entry| {
		!candidates
			.iter()
			.any(|candidate| candidate.seq == entry.seq)
	});
	let window = budget(usize::MAX)
		.request(&retained, &pinned)
		.estimated_total_tokens()
		+ 2_000;
	let budget = budget(window);
	let policy = recovery_policy(true);
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	// Act
	let plan = *super::super::summary_plan(context.clone(), &fitting).unwrap();
	let (adopted, _) = summary_candidate(
		&plan,
		&summary_text(&[], &[], &[]),
		Default::default(),
		summarizer(),
		1024,
		&fitting,
	)
	.unwrap();
	// Assert
	assert_eq!(plan.absorbed.len() % 3, 0, "only whole turns are absorbed");
	let absorbed_turns = plan.absorbed.len() / 3;
	assert!((6..12).contains(&absorbed_turns));
	assert_eq!(
		rendered_turns(&adopted),
		(absorbed_turns..12).map(turn_ids).collect::<Vec<_>>()
	);
	assert!(adopted.execution_summary.is_some());
	assert!(matches!(
		adopted.events().next(),
		Some(ContextEvent::Human { .. })
	));
}

#[test]
fn fitting_falls_back_to_the_newest_turn_before_pausing() {
	let context = turns(3);
	let pinned = json!({"task": {"description": "x".repeat(500_000)}});
	let budget = budget(100_000);
	let policy = recovery_policy(true);
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	let result = super::super::compact(&context, &fitting, 0).unwrap();
	let Compaction::NeedsSummary(plan, _) = result else {
		panic!("older turns remain reducible")
	};
	assert_eq!(plan.absorbed.len(), 6);
}

#[rstest::rstest]
fn a_turn_with_a_verbatim_result_is_never_absorbed_in_part() {
	// Arrange
	let mut absorbed = turns(3).history[1..].to_vec();
	// Act: the second turn's first result must stay verbatim.
	retain_summarizable_turns(
		&mut absorbed,
		|event| !matches!(event, ContextEvent::Tool { call, .. } if call.id == "t1-a"),
	);
	// Assert
	assert_eq!(
		absorbed.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
		[2, 3, 4, 8, 9, 10]
	);
}

/// `turns(count)` with each turn's reasoning recorded (#942).
fn turns_with_reasoning(count: usize) -> Context {
	let mut context = turns(count);
	for entry in &mut context.history {
		if let ContextEvent::Assistant {
			epoch, reasoning, ..
		} = &mut entry.event
		{
			*reasoning = Some(provider::reasoning::Reasoning::ChatDetails {
				details: vec![
					json!({"type":"reasoning.text","text":format!("thinking {epoch}"),"index":0}),
				],
			});
		}
	}
	context
}

/// Whether each rendered Model Turn, oldest first, still carries reasoning.
fn reasoning_by_turn(context: &Context) -> Vec<bool> {
	native_context(context, &json!({}))
		.history
		.iter()
		.filter_map(|message| match message {
			NativeMessage::Assistant { reasoning, .. } => Some(reasoning.is_some()),
			_ => None,
		})
		.collect()
}

#[test]
fn an_ordinary_step_keeps_every_turns_reasoning() {
	let context = turns_with_reasoning(10);
	let pinned = json!({});
	let budget = budget(1_000_000);
	let policy = recovery_policy(true);
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	assert!(matches!(
		super::super::compact(&context, &fitting, 0).unwrap(),
		Compaction::Fits
	));
	assert!(reasoning_by_turn(&context).iter().all(|kept| *kept));
}

#[test]
fn a_compaction_step_drops_reasoning_only_before_the_protected_tail() {
	// Arrange: a request just over the window must be reduced.
	let context = turns_with_reasoning(10);
	let pinned = json!({});
	let full = budget(usize::MAX)
		.request(&context, &pinned)
		.estimated_total_tokens();
	let budget = budget(full - 1);
	let policy = recovery_policy(true);
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	// Act
	let compacted = match super::super::compact(&context, &fitting, 0).unwrap() {
		Compaction::Offloaded { context, .. } => *context,
		Compaction::NeedsSummary(plan, _) => plan.pruned,
		Compaction::Fits => panic!("the request does not fit"),
	};
	// Assert: older turns lost their reasoning, the protected tail and the
	// newest turn kept theirs, and the journal-backed original is untouched.
	let kept = reasoning_by_turn(&compacted);
	assert_eq!(kept.len(), 10);
	assert!(!kept[0], "the oldest turn's reasoning is dropped");
	assert!(kept[9], "the newest turn keeps its reasoning");
	let first_kept = kept.iter().position(|kept| *kept).unwrap();
	assert!(kept[first_kept..].iter().all(|kept| *kept));
	assert!(compacted.compactions > context.compactions);
	assert!(reasoning_by_turn(&context).iter().all(|kept| *kept));
}

#[test]
fn turns_a_summary_absorbs_carry_no_reasoning_into_it() {
	let context = turns_with_reasoning(3);
	let pinned = json!({"task": {"description": "x".repeat(500_000)}});
	let budget = budget(100_000);
	let policy = recovery_policy(true);
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	let Compaction::NeedsSummary(plan, _) = super::super::compact(&context, &fitting, 0).unwrap()
	else {
		panic!("older turns remain reducible")
	};
	assert!(!plan.absorbed.is_empty());
	assert!(plan.absorbed.iter().all(|entry| !matches!(
		&entry.event,
		ContextEvent::Assistant {
			reasoning: Some(_),
			..
		}
	)));
	assert_eq!(reasoning_by_turn(&plan.pruned).last(), Some(&true));
}
