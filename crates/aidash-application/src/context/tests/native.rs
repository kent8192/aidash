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
			epoch: turn as i64,
			text: if turn % 2 == 0 {
				format!("Step {turn}")
			} else {
				String::new()
			},
			calls: ids.to_vec(),
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

#[rstest::rstest]
fn a_turn_that_renders_nothing_takes_no_protected_slot() {
	// Arrange: the latest response's only call was deferred before it ran.
	let mut context = turns(10);
	context.push(ContextEvent::Assistant {
		epoch: 10,
		text: String::new(),
		calls: vec!["deferred".into()],
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
	let mut context = turns(12);
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
	let plan = needs_summary(
		super::super::compact_decided(&mut context, None, &fitting)
			.await
			.unwrap(),
	);
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
	assert_eq!(plan.absorbed.len(), 18, "six whole turns of three entries");
	assert_eq!(
		rendered_turns(&adopted),
		(6..12).map(turn_ids).collect::<Vec<_>>()
	);
	assert!(adopted.execution_summary.is_some());
	assert!(matches!(
		adopted.events().next(),
		Some(ContextEvent::Human { .. })
	));
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
