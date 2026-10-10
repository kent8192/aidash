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
		"verification":[{"reference":"call_7","outcome":"cargo test passed"}]
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
		SummaryContent::parse(text, previous, 4096).unwrap(),
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
			4096
		),
		Err(Rejection::DroppedItem("c1".into()))
	);
	assert_eq!(
		SummaryContent::parse(
			&content(&[("c1", "never touch main")], &[], &[]),
			Some(&first),
			4096
		),
		Err(Rejection::DroppedItem("u1".into()))
	);
	// Explicit resolution closes an item; the merged range keeps its origin.
	let second = adopted(
		&content(
			&[("c1", "never touch main")],
			&[],
			&[("u1", "call_9 fixed it")],
		),
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
		SummaryContent::parse("not json", None, 4096),
		Err(Rejection::Malformed)
	);
	let empty_goal = content(&[], &[], &[]).replace("ship the parser", " ");
	assert_eq!(
		SummaryContent::parse(&empty_goal, None, 4096),
		Err(Rejection::EmptyGoal)
	);
	assert_eq!(
		SummaryContent::parse(&content(&[("a", "x"), ("a", "y")], &[], &[]), None, 4096),
		Err(Rejection::InvalidItem)
	);
	assert_eq!(
		SummaryContent::parse(&content(&[], &[], &[]), None, 16),
		Err(Rejection::Oversized(16))
	);
	let extra = content(&[], &[], &[]).replacen('{', "{\"note\":\"x\",", 1);
	assert_eq!(
		SummaryContent::parse(&extra, None, 4096),
		Err(Rejection::Malformed)
	);
}

#[test]
fn merge_rejects_overlapping_ranges() {
	let first = adopted(&content(&[], &[], &[]), None, &[tool(2), tool(3)]);
	let parsed = SummaryContent::parse(&content(&[], &[], &[]), Some(&first), 4096).unwrap();
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
