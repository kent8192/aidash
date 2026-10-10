use super::*;
use crate::provider::ToolCall;

fn content(
	constraints: &[(&str, &str)],
	unresolved: &[(&str, &str)],
	resolved: &[(&str, &str)],
) -> String {
	let items = |items: &[(&str, &str)]| {
		items
			.iter()
			.map(|(id, text)| json!({"id":id,"text":text}))
			.collect::<Vec<_>>()
	};
	json!({
		"goal":"ship the parser",
		"constraints":items(constraints),
		"decisions":["use nom"],
		"unresolved":items(unresolved),
		"resolved":resolved.iter().map(|(id, by)| json!({"id":id,"resolved_by":by})).collect::<Vec<_>>(),
		"artifacts":[{"reference":"src/parser.rs","revision":""}],
		"verification":[{"reference":"call_2","outcome":"cargo test passed"}]
	})
	.to_string()
}

fn tool(seq: u64) -> HistoryEntry {
	HistoryEntry {
		seq,
		event: ContextEvent::tool(
			ToolCall {
				id: format!("call_{seq}"),
				name: "file_read".into(),
				arguments: json!({}),
			},
			json!({"ok":true}),
		),
	}
}

fn provider() -> SummaryProvider {
	SummaryProvider {
		model: EntityRef {
			id: "summarizer".into(),
			version: "1".into(),
		},
		definition_digest: "d".into(),
	}
}

fn adopted(
	text: &str,
	previous: Option<&ExecutionSummary>,
	absorbed: &[HistoryEntry],
) -> ExecutionSummary {
	ExecutionSummary::merge(
		SummaryContent::parse(text, previous, absorbed, 4096).unwrap(),
		previous,
		absorbed,
		SummaryDependencies::default(),
		"context-recovery/1",
		provider(),
	)
	.unwrap()
}

#[test]
fn merge_keeps_constraints_and_unresolved_work_across_compactions() {
	let first = adopted(
		&content(
			&[("c1", "never touch main")],
			&[("u1", "fix flaky test")],
			&[],
		),
		None,
		&[tool(2), tool(3)],
	);
	assert_eq!((first.source.from_seq, first.source.through_seq), (2, 3));
	// Dropping a constraint or open item silently is rejected.
	assert_eq!(
		SummaryContent::parse(
			&content(&[], &[("u1", "fix flaky test")], &[]),
			Some(&first),
			&[tool(5)],
			4096
		),
		Err(Rejection::DroppedItem("c1".into()))
	);
	assert_eq!(
		SummaryContent::parse(
			&content(&[("c1", "never touch main")], &[], &[]),
			Some(&first),
			&[tool(5)],
			4096
		),
		Err(Rejection::DroppedItem("u1".into()))
	);
	// A retained ID must keep its list and exact text; rewriting or moving it
	// would silently drop the original requirement from model context.
	assert_eq!(
		SummaryContent::parse(
			&content(
				&[("c1", "touch main freely")],
				&[("u1", "fix flaky test")],
				&[]
			),
			Some(&first),
			&[tool(5)],
			4096
		),
		Err(Rejection::ChangedItem("c1".into()))
	);
	assert_eq!(
		SummaryContent::parse(
			&content(
				&[("c1", "never touch main"), ("u1", "fix flaky test")],
				&[],
				&[]
			),
			Some(&first),
			&[tool(5)],
			4096
		),
		Err(Rejection::ChangedItem("u1".into()))
	);
	// Explicit resolution closes an item; the merged range keeps its origin.
	let second = adopted(
		&content(&[("c1", "never touch main")], &[], &[("u1", "call_5")]),
		Some(&first),
		&[tool(5)],
	);
	assert_eq!((second.source.from_seq, second.source.through_seq), (2, 5));
	assert_eq!(second.previous.as_ref().unwrap().digest, first.digest);
	assert_eq!(second.content.constraints[0].id, "c1");
}

#[test]
fn invalid_candidates_are_rejected() {
	assert_eq!(
		SummaryContent::parse("not json", None, &[tool(2)], 4096),
		Err(Rejection::Malformed)
	);
	let empty_goal = content(&[], &[], &[]).replace("ship the parser", " ");
	assert_eq!(
		SummaryContent::parse(&empty_goal, None, &[tool(2)], 4096),
		Err(Rejection::EmptyGoal)
	);
	assert_eq!(
		SummaryContent::parse(
			&content(&[("a", "x"), ("a", "y")], &[], &[]),
			None,
			&[tool(2)],
			4096
		),
		Err(Rejection::InvalidItem)
	);
	assert_eq!(
		SummaryContent::parse(&content(&[], &[], &[]), None, &[tool(2)], 16),
		Err(Rejection::Oversized(16))
	);
	let extra = content(&[], &[], &[]).replacen('{', "{\"note\":\"x\",", 1);
	assert_eq!(
		SummaryContent::parse(&extra, None, &[tool(2)], 4096),
		Err(Rejection::Malformed)
	);
}

#[test]
fn merge_rejects_overlapping_ranges() {
	let first = adopted(&content(&[], &[], &[]), None, &[tool(2), tool(3)]);
	let parsed =
		SummaryContent::parse(&content(&[], &[], &[]), Some(&first), &[tool(4)], 4096).unwrap();
	assert!(
		ExecutionSummary::merge(
			parsed,
			Some(&first),
			&[tool(3)],
			SummaryDependencies::default(),
			"v",
			provider()
		)
		.is_err()
	);
}

#[test]
fn only_tool_and_media_events_are_absorbable() {
	assert!(absorbable(&tool(1).event));
	assert!(absorbable(&ContextEvent::ModelMediaObservation {
		text: "x".into(),
		through_seq: None,
		truncated: false
	}));
	assert!(!absorbable(&ContextEvent::Human {
		request: "r".into(),
		request_kind: "question".into(),
		response: json!("no")
	}));
	assert!(!absorbable(&ContextEvent::RunMessageReadRequired {
		message_ids: vec![]
	}));
}

#[test]
fn ordered_projection_carries_the_adopted_summary_before_history() {
	let mut context = crate::context::Context::default();
	context.push(tool(1).event);
	let pinned = json!({"task":{"id":"t"}});
	// Prune-only contexts keep the Ordered Stable Prefix bytes unchanged.
	let prune_only = crate::context::ordered_context(&context, &pinned);
	assert!(!prune_only.stable.contains("\"summary\""));
	context.execution_summary = Some(Box::new(adopted(
		&content(&[("c1", "never touch main")], &[], &[]),
		None,
		&[tool(2)],
	)));
	let summarized = crate::context::ordered_context(&context, &pinned);
	let stable: serde_json::Value = serde_json::from_str(&summarized.stable).unwrap();
	assert_eq!(stable["summary"]["constraints"][0]["id"], "c1");
	assert!(summarized.stable.find("\"summary\"") < summarized.stable.find("\"history\""));
	assert_eq!(summarized.volatile, prune_only.volatile);
}

#[test]
fn resolutions_and_verification_need_tool_call_evidence() {
	let first = adopted(
		&content(
			&[("c1", "never touch main")],
			&[("u1", "fix flaky test")],
			&[],
		),
		None,
		&[tool(2), tool(3)],
	);
	// A resolution must name a tool call this merge absorbs, not free text or
	// a call outside it.
	for by in ["call_5 fixed it", "call_9", "call_2"] {
		assert_eq!(
			SummaryContent::parse(
				&content(&[("c1", "never touch main")], &[], &[("u1", by)]),
				Some(&first),
				&[tool(5)],
				4096
			),
			Err(Rejection::UnprovenResolution("u1".into()))
		);
	}
	// An invented verification reference is never adopted.
	let invented = content(&[], &[], &[]).replace("call_2", "call_999");
	assert_eq!(
		SummaryContent::parse(&invented, None, &[tool(2)], 4096),
		Err(Rejection::UnprovenVerification("call_999".into()))
	);
	// A previous verification is carried only unchanged.
	let rewritten = content(
		&[("c1", "never touch main")],
		&[("u1", "fix flaky test")],
		&[],
	)
	.replace("cargo test passed", "all checks passed");
	assert_eq!(
		SummaryContent::parse(&rewritten, Some(&first), &[tool(5)], 4096),
		Err(Rejection::UnprovenVerification("call_2".into()))
	);
	assert!(
		SummaryContent::parse(
			&content(
				&[("c1", "never touch main")],
				&[("u1", "fix flaky test")],
				&[]
			),
			Some(&first),
			&[tool(5)],
			4096
		)
		.is_ok()
	);
}

#[test]
fn absorbed_ranges_skip_entries_pruned_between_merges() {
	let first = adopted(&content(&[], &[], &[]), None, &[tool(2), tool(3), tool(5)]);
	let second = adopted(&content(&[], &[], &[]), Some(&first), &[tool(6), tool(9)]);
	assert_eq!(second.source.absorbed, vec![[2, 3], [5, 6], [9, 9]]);
	assert_eq!((second.source.from_seq, second.source.through_seq), (2, 9));
	for (seq, absorbed) in [(2, true), (4, false), (6, true), (7, false), (9, true)] {
		assert_eq!(second.source.absorbs(seq), absorbed, "{seq}");
	}
	let parsed = SummaryContent::parse(&content(&[], &[], &[]), None, &[tool(2)], 4096).unwrap();
	assert!(
		ExecutionSummary::merge(
			parsed,
			None,
			&[tool(3), tool(2)],
			SummaryDependencies::default(),
			"v",
			provider()
		)
		.is_err()
	);
}
