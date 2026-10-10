use super::*;
use crate::ports::{CompactionClassifier as JevAsker, CompactionQuestions as Questions};
use aidash_domain::context::*;
use aidash_domain::provider;

use async_trait::async_trait;
use std::sync::Mutex;

struct FakeJev {
	answer: fn(&str) -> Value,
	seen: Mutex<Vec<(Value, Questions)>>,
}
impl FakeJev {
	fn new(answer: fn(&str) -> Value) -> Self {
		Self {
			answer,
			seen: Mutex::new(Vec::new()),
		}
	}
}
#[async_trait]
impl JevAsker for FakeJev {
	async fn ask(&self, state: &Value, questions: &Questions) -> Result<Value> {
		self.seen
			.lock()
			.unwrap()
			.push((state.clone(), questions.clone()));
		let answers: serde_json::Map<_, _> = questions
			.keys()
			.map(|key| (key.clone(), (self.answer)(key)))
			.collect();
		Ok(json!({"answers":answers}))
	}
}
fn drop_all(_: &str) -> Value {
	json!({"noul":0.0})
}
fn tool(id: &str, result: &str) -> ContextEvent {
	event(
		json!({"kind":"tool","call":{"id":id,"name":"read","arguments":{"path":id}},"result":result}),
	)
}
fn history() -> Vec<ContextEvent> {
	let mut history = vec![
		tool("first", "pinned first result"),
		tool("obsolete", &"old".repeat(3000)),
		ContextEvent::Human {
			request: "Never deploy".into(),
			request_kind: "APPROVAL_REQUIRED".into(),
			response: json!({"approved":false}),
		},
		ContextEvent::ModelMediaObservation {
			text: "Preserve this exact path: src/generated".into(),
			through_seq: None,
			truncated: false,
		},
	];
	for i in 0..6 {
		history.push(tool(&format!("recent-{i}"), "recent"));
	}
	history
}

#[rstest::rstest]
#[tokio::test]
async fn japanese_history_compacts_before_the_final_request_check() {
	let mut context = Context {
		history: history(),
		..Default::default()
	};
	context.history[1] = tool("obsolete", &"日".repeat(4000));
	let asker = FakeJev::new(drop_all);
	let pinned = json!({"task":"航空会社の新規事業計画"});
	compact(&mut context, &asker, 12000, &pinned, "")
		.await
		.unwrap();
	let request = provider::ModelRequest {
		instructions: String::new(),
		context: json!({"current":pinned,"summary":context.summary,"history":context.history})
			.into(),
		tools: vec![],
		max_output_tokens: 256,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
	};
	check_request(12000, &request).unwrap();
	assert_eq!(context.compactions, 1);
}

#[rstest::rstest]
#[tokio::test]
async fn media_space_is_reserved_before_retained_history_is_compacted() {
	let media = vec![provider::ContentPart::Image {
		media_type: "image/png".into(),
		bytes: vec![0; 8192],
	}];
	let window = 12_000;
	let budget = RequestBudget {
		window: window - provider::ModelRequest::content_parts_reservation(&media),
		instructions: "Inspect the image",
		tools: &[],
		max_output_tokens: 256,
		projection: &RequestProjection::Legacy,
	};
	let pinned = json!({"task":"Inspect accepted image"});
	let mut context = Context {
		history: history(),
		..Default::default()
	};
	context.history[1] = tool("obsolete", &"old".repeat(6000));
	super::compact(&mut context, &FakeJev::new(drop_all), &budget, &pinned)
		.await
		.unwrap();
	let mut request = budget.request(&context, &pinned);
	request.content_parts = media;
	check_request(window, &request).unwrap();
	assert_eq!(context.compactions, 1);
}

#[rstest::rstest]
fn request_check_reserves_completion_tokens() {
	let request = provider::ModelRequest {
		instructions: String::new(),
		context: json!({}).into(),
		tools: vec![],
		max_output_tokens: 4096,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
	};
	assert!(check_request(2000, &request).is_err());
}

#[rstest::rstest]
fn request_check_reserves_the_registered_model_maximum_with_input_and_framing() {
	let budget = RequestBudget {
		window: 1_048_576,
		instructions: "",
		tools: &[],
		max_output_tokens: 65_536,
		projection: &RequestProjection::Legacy,
	};
	let request = budget.request(&Context::default(), &json!({}));
	assert_eq!(request.max_output_tokens, 65_536);
	assert!(check_request(66_000, &request).is_err());
	assert!(check_request(67_000, &request).is_ok());
}

#[rstest::rstest]
fn tool_event_growth_matches_the_complete_request_delta() {
	use provider::ToolSpec;
	let tools = vec![ToolSpec {
		name: "workspace_read".into(),
		description: "Read a record".into(),
		parameters: json!({"type":"object","properties":{"id":{"type":"string"}}}),
	}];
	let pinned = json!({"private":"日本語 \"quoted\" \\ escaped context"});
	let context = Context {
		summary: "summary with text".into(),
		history: vec![tool("previous", "result")],
		..Default::default()
	};
	let event = tool(
		"read",
		&json!({"content":"quotes \" and backslashes \\ and 日本語"}).to_string(),
	);
	let mut after = context.clone();
	after.history.push(event.clone());
	let budget = RequestBudget {
		window: usize::MAX,
		instructions: "instructions with newline\n",
		tools: &tools,
		max_output_tokens: 2048,
		projection: &RequestProjection::Legacy,
	};
	let delta = budget
		.request(&after, &pinned)
		.estimated_total_tokens()
		.saturating_sub(budget.request(&context, &pinned).estimated_total_tokens());
	assert_eq!(tool_event_growth(&context, &event), delta);
}

#[rstest::rstest]
#[tokio::test]
async fn fitting_and_final_checks_share_escaped_input_tools_and_output_budget() {
	use aidash_domain::provider::ToolSpec;
	for text in ["ASCII", "日本語", "\"\\\n\t"] {
		let tools = vec![ToolSpec {
			name: "lookup".into(),
			description: text.repeat(200),
			parameters: json!({"type":"object","properties":{"query":{"type":"string"}}}),
		}];
		let pinned = json!({"task":text.repeat(50)});
		let mut context = Context {
			history: vec![tool("first", text)],
			..Default::default()
		};
		let mut budget = RequestBudget {
			window: usize::MAX,
			instructions: text,
			tools: &tools,
			max_output_tokens: 4096,
			projection: &RequestProjection::Legacy,
		};
		let request = budget.request(&context, &pinned);
		budget.window = request.estimated_total_tokens();
		let asker = FakeJev::new(drop_all);
		super::compact(&mut context, &asker, &budget, &pinned)
			.await
			.unwrap();
		check_request(budget.window, &budget.request(&context, &pinned)).unwrap();
		let before = json!(context);
		budget.window -= 1;
		assert!(check_request(budget.window, &request).is_err());
		assert!(
			super::compact(&mut context, &asker, &budget, &pinned)
				.await
				.is_err()
		);
		assert_eq!(json!(context), before);
		assert!(asker.seen.lock().unwrap().is_empty());
	}
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_uses_jev_without_rewriting_text_or_legacy_summaries() {
	let mut context = Context {
		summary: "Legacy summary remains verbatim".into(),
		history: history(),
		..Default::default()
	};
	let before = context.history.clone();
	let asker = FakeJev::new(drop_all);
	compact(
		&mut context,
		&asker,
		4000,
		&json!({"task":{"description":"Fix the test"}}),
		"Keep secrets private",
	)
	.await
	.unwrap();
	assert_eq!(context.compactions, 1);
	assert_eq!(context.summary, "Legacy summary remains verbatim");
	assert_eq!(context.history, [&before[..1], &before[2..]].concat());
	let seen = asker.seen.lock().unwrap();
	assert_eq!(seen.len(), 1);
	assert_eq!(seen[0].1.len(), 2);
	let state = seen[0].0.to_string();
	assert!(state.contains("Keep secrets private"));
	assert!(state.contains("Never deploy"));
	assert!(state.contains("recent-5"));
	assert!(!state.contains(&"old".repeat(100)));
}

#[rstest::rstest]
#[tokio::test]
async fn failed_or_insufficient_compaction_never_mutates_context() {
	for answer in [
		drop_all as fn(&str) -> Value,
		|_| json!({"noul":1.0}),
		|_| json!({"noul":"invalid"}),
	] {
		let mut context = Context {
			history: history(),
			..Default::default()
		};
		let before = json!(context);
		assert!(
			compact(&mut context, &FakeJev::new(answer), 10, &json!({}), "")
				.await
				.is_err()
		);
		assert_eq!(json!(context), before);
	}
}

#[rstest::rstest]
#[tokio::test]
async fn short_runs_and_pinned_only_histories_never_call_jev() {
	let asker = FakeJev::new(drop_all);
	let mut context = Context {
		history: vec![tool("only", "small")],
		..Default::default()
	};
	compact(&mut context, &asker, 2000, &json!({}), "")
		.await
		.unwrap();
	assert_eq!(context.compactions, 0);
	assert!(
		compact(&mut context, &asker, 1, &json!({}), "")
			.await
			.is_err()
	);
	assert!(asker.seen.lock().unwrap().is_empty());
}

#[rstest::rstest]
#[tokio::test]
async fn decisions_keep_pairs_truncate_results_and_drop_only_obsolete_pairs() {
	let mut events = vec![
		ContextEvent::Human {
			request: "original instruction".into(),
			request_kind: "INFORMATION_REQUEST".into(),
			response: Value::Null,
		},
		tool("drop", &"discard".repeat(200)),
		tool("truncate", &"日".repeat(1000)),
		tool("keep-result", "exact retained output"),
		tool("short", "short result"),
		ContextEvent::ModelMediaObservation {
			text: "Pending read".into(),
			through_seq: None,
			truncated: false,
		},
		ContextEvent::Human {
			request: "rejected approval".into(),
			request_kind: "INFORMATION_REQUEST".into(),
			response: Value::Null,
		},
	];
	events.extend((0..6).map(|i| tool(&format!("recent-{i}"), "pinned")));
	let asker = FakeJev::new(|name| {
		let probability = match name {
			"call_t2" | "call_t4" => 0.5,
			"result_t3" => 0.5,
			_ => 0.0,
		};
		json!({"noul":probability})
	});
	let output = compaction::prune(&events, &json!({}), &asker, &compaction::Options::default())
		.await
		.unwrap();
	assert_eq!(output.calls_dropped, 1);
	assert_eq!(output.results_truncated, 1);
	assert_eq!(output.history.len(), events.len() - 1);
	assert_eq!(output.history[0], events[0]);
	assert_eq!(json!(output.history[1])["call"], json!(events[2])["call"]);
	assert!(
		json!(output.history[1])["result"]
			.as_str()
			.unwrap()
			.starts_with(&"日".repeat(300))
	);
	assert!(
		json!(output.history[1])["result"]
			.as_str()
			.unwrap()
			.contains("700 chars")
	);
	assert_eq!(output.history[2..], events[3..]);
}

#[rstest::rstest]
#[tokio::test]
async fn batches_resend_the_same_state_and_fit_the_request_budget() {
	let mut events = vec![ContextEvent::Human {
		request: "Do the task".into(),
		request_kind: "INFORMATION_REQUEST".into(),
		response: Value::Null,
	}];
	events.extend((0..12).map(|i| tool(&format!("old-{i}"), "output")));
	let asker = FakeJev::new(drop_all);
	let mut options = compaction::Options {
		preserve_recent: 0,
		..Default::default()
	};
	compaction::prune(&events, &json!({}), &asker, &options)
		.await
		.unwrap();
	let state = asker.seen.lock().unwrap()[0].0.clone();
	// Room for about one call's two questions, with the same full state in each request.
	options.max_request_tokens =
		compaction::estimate_tokens(&json!({"state":state,"questions":{}}).to_string()) + 260;
	let asker = FakeJev::new(drop_all);
	let output = compaction::prune(&events, &json!({}), &asker, &options)
		.await
		.unwrap();
	{
		let seen = asker.seen.lock().unwrap();
		assert!(seen.len() > 1);
		assert_eq!(seen.len(), output.requests);
		let mut names = std::collections::BTreeSet::new();
		for (actual_state, questions) in seen.iter() {
			assert_eq!(actual_state, &state);
			let request = json!({"model":"jev-latest","state":actual_state,"questions":questions});
			assert!(
				compaction::estimate_tokens(&request.to_string()) <= options.max_request_tokens
			);
			for name in questions.keys() {
				assert!(names.insert(name.clone()));
			}
		}
		assert_eq!(names.len(), 24);
	}
	assert_eq!(output.history, events[..1]);
	options.max_request_tokens = 1;
	assert!(
		compaction::prune(&events, &json!({}), &FakeJev::new(drop_all), &options)
			.await
			.is_err()
	);
}

#[rstest::rstest]
#[tokio::test]
async fn state_fitting_shrinks_only_the_classification_view() {
	let mut events = vec![ContextEvent::Human {
		request: "first instruction".into(),
		request_kind: "INFORMATION_REQUEST".into(),
		response: Value::Null,
	}];
	for i in 0..40 {
		let mut event = tool(&format!("tool-{i}"), "SECRET RESULT CONTENT");
		if let ContextEvent::Tool { call, .. } = &mut event {
			call.arguments["content"] = json!("content ".repeat(1000));
		}
		events.push(event);
	}
	events.push(ContextEvent::Human {
		request: "last instruction".into(),
		request_kind: "INFORMATION_REQUEST".into(),
		response: Value::Null,
	});
	let original = events.clone();
	let asker = FakeJev::new(|_| json!({"noul":1.0}));
	let options = compaction::Options {
		max_state_tokens: 1800,
		preserve_recent: 1,
		..Default::default()
	};
	let output = compaction::prune(&events, &json!({}), &asker, &options)
		.await
		.unwrap();
	assert_eq!(output.history, original);
	assert_ne!(output.stage, "full");
	for (state, _) in asker.seen.lock().unwrap().iter() {
		let serialized = state.to_string();
		assert!(compaction::estimate_tokens(&serialized) <= options.max_state_tokens);
		assert!(serialized.contains("first instruction"));
		assert!(serialized.contains("last instruction"));
		assert!(!serialized.contains("SECRET RESULT CONTENT"));
		for i in 1..=40 {
			assert!(serialized.contains(&format!("t{i}")));
		}
	}
	let impossible = compaction::Options {
		max_state_tokens: 1,
		..options
	};
	assert!(
		compaction::prune(&events, &json!({}), &asker, &impossible)
			.await
			.is_err()
	);
}

#[rstest::rstest]
fn jev_token_estimate_matches_upstream_examples() {
	for (input, tokens) in [
		("", 0),
		("hello world", 2),
		("internationalization", 4),
		("12345678", 4),
	] {
		assert_eq!(compaction::estimate_tokens(input), tokens);
	}
}

#[rstest::rstest]
fn large_optional_workspace_snapshot_fits_without_changing_task_identity() {
	let id = uuid::Uuid::new_v4().to_string();
	let mut pinned = json!({"task":{"id":id,"description":"important task"},"workspace":{"tasks":(0..300).map(|_|json!({"description":"x".repeat(10000)})).collect::<Vec<_>>()},"memory":"y".repeat(100000)});
	bound_snapshot(&mut pinned, 2048).unwrap();
	assert_eq!(pinned["task"]["id"], id);
	assert_eq!(pinned["snapshot_truncated"], true);
	assert!(estimated_tokens(&pinned.to_string()) <= 2048);
}

#[rstest::rstest]
fn snapshot_budget_also_bounds_wide_state_objects() {
	let state: serde_json::Map<String, Value> = (0..4000)
		.map(|n| (format!("field-{n}"), json!(true)))
		.collect();
	let mut pinned = json!({"identity":{"agent_id":"research"},"task":{"id":"task-id"},"workspace":{"state":state}});
	bound_snapshot(&mut pinned, 2000).unwrap();
	assert!(estimated_tokens(&pinned.to_string()) <= 2000);
	assert_eq!(pinned["task"]["id"], "task-id");
	assert_eq!(pinned["snapshot_truncated"], true);
}

#[rstest::rstest]
fn registration_and_execution_share_the_context_reserve_at_its_boundary() {
	let instructions = "Use the private reference documents.";
	let specifications = vec![];
	let output = 4096;
	let private_context = json!({"reference_documents":"private \"quoted\" reference"});
	let request = RequestBudget {
		window: 0,
		instructions,
		tools: &specifications,
		max_output_tokens: output,
		projection: &RequestProjection::Legacy,
	};
	let window = request
		.request(&Context::default(), &private_context)
		.estimated_total_tokens()
		+ MIN_CONTEXT_RESERVE;
	let registration_budget = request_context_budget(
		window,
		output,
		instructions,
		&specifications,
		&private_context,
		&RequestProjection::Legacy,
	)
	.unwrap();
	let execution_budget =
		RequestBudget { window, ..request }.remaining(&Context::default(), &private_context);
	assert_eq!(registration_budget, MIN_CONTEXT_RESERVE);
	assert_eq!(execution_budget, registration_budget);
	assert!(
		request_context_budget(
			window - 1,
			output,
			instructions,
			&specifications,
			&private_context,
			&RequestProjection::Legacy
		)
		.is_err()
	);
}

#[rstest::rstest]
fn compaction_snapshot_redacts_private_documents_without_mutating_inference_context() {
	let pinned = json!({
		"task":{"title":"Summarize references"},
		"reference_documents":[{"text":"PRIVATE-REFERENCE-123"}],
	});
	let redacted = compaction_snapshot(&pinned);
	assert!(redacted.get("reference_documents").is_none());
	assert!(!redacted.to_string().contains("PRIVATE-REFERENCE-123"));
	assert_eq!(redacted["task"]["title"], "Summarize references");
	assert_eq!(
		pinned["reference_documents"][0]["text"],
		"PRIVATE-REFERENCE-123"
	);
}

// Exercise the same complete-request fitting path as the harness.
async fn compact(
	context: &mut Context,
	asker: &dyn JevAsker,
	window: usize,
	pinned: &Value,
	instructions: &str,
) -> Result<()> {
	super::compact(
		context,
		asker,
		&RequestBudget {
			window,
			instructions,
			tools: &[],
			max_output_tokens: 256,
			projection: &RequestProjection::Legacy,
		},
		pinned,
	)
	.await
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_counts_private_documents_without_disclosing_them() {
	let mut context = Context {
		history: history(),
		..Default::default()
	};
	let asker = FakeJev::new(drop_all);
	let pinned =
		json!({"task":"Summarize", "reference_documents":"PRIVATE-REFERENCE-123".repeat(50)});
	let budget = RequestBudget {
		window: 6000,
		instructions: "",
		tools: &[],
		max_output_tokens: 256,
		projection: &RequestProjection::Legacy,
	};
	super::compact(&mut context, &asker, &budget, &pinned)
		.await
		.unwrap();
	assert_eq!(context.compactions, 1);
	let seen = asker.seen.lock().unwrap();
	assert!(!seen.is_empty());
	assert!(
		seen.iter()
			.all(|(state, _)| !state.to_string().contains("PRIVATE-REFERENCE-123"))
	);
	assert!(budget.request(&context, &pinned).estimated_total_tokens() <= budget.window);
	assert!(
		serde_json::to_string(&budget.request(&context, &pinned).context)
			.unwrap()
			.contains("PRIVATE-REFERENCE-123")
	);
}

fn event(value: Value) -> ContextEvent {
	serde_json::from_value(value).unwrap()
}
fn check_request(window: usize, request: &provider::ModelRequest) -> aidash_domain::Result<()> {
	request.ensure_fits(window)
}
