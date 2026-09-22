use super::*;
use async_trait::async_trait;
use std::sync::Mutex;

struct FakeJev {
	answer: fn(&str) -> Value,
	seen: Mutex<Vec<(Value, jev::Questions)>>,
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
impl jev::JevAsker for FakeJev {
	async fn ask(&self, state: &Value, questions: &jev::Questions) -> Result<Value> {
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
fn tool(id: &str, result: &str) -> Value {
	json!({"kind":"tool","call":{"id":id,"name":"read","arguments":{"path":id}},"result":result})
}
fn history() -> Vec<Value> {
	let mut history = vec![
		tool("first", "pinned first result"),
		tool("obsolete", &"old".repeat(3000)),
		json!({"kind":"human","response":{"approved":false},"prompt":"Never deploy"}),
		json!({"kind":"assistant","text":"Preserve this exact path: src/generated"}),
	];
	for i in 0..6 {
		history.push(tool(&format!("recent-{i}"), "recent"));
	}
	history
}

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

#[tokio::test]
async fn decisions_keep_pairs_truncate_results_and_drop_only_obsolete_pairs() {
	let mut events = vec![
		json!({"kind":"human","content":"original instruction"}),
		tool("drop", &"discard".repeat(200)),
		tool("truncate", &"日".repeat(1000)),
		tool("keep-result", "exact retained output"),
		tool("short", "short result"),
		json!({"kind":"tool","call":{"id":"pending","name":"read","arguments":{}}}),
		json!({"kind":"human","content":"rejected approval"}),
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
	assert_eq!(output.history[1]["call"], events[2]["call"]);
	assert!(
		output.history[1]["result"]
			.as_str()
			.unwrap()
			.starts_with(&"日".repeat(300))
	);
	assert!(
		output.history[1]["result"]
			.as_str()
			.unwrap()
			.contains("700 chars")
	);
	assert_eq!(output.history[2..], events[3..]);
}

#[tokio::test]
async fn batches_resend_the_same_state_and_fit_the_request_budget() {
	let mut events = vec![json!({"kind":"human","text":"Do the task"})];
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

#[tokio::test]
async fn state_fitting_shrinks_only_the_classification_view() {
	let mut events = vec![json!({"kind":"human","text":"first instruction"})];
	for i in 0..40 {
		let mut event = tool(&format!("tool-{i}"), "SECRET RESULT CONTENT");
		event["call"]["arguments"]["content"] = json!("content ".repeat(1000));
		events.push(event);
	}
	events.push(json!({"kind":"human","text":"last instruction"}));
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

#[test]
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

#[test]
fn large_optional_workspace_snapshot_fits_without_changing_task_identity() {
	let id = uuid::Uuid::new_v4().to_string();
	let mut pinned = json!({"task":{"id":id,"description":"important task"},"workspace":{"tasks":(0..300).map(|_|json!({"description":"x".repeat(10000)})).collect::<Vec<_>>()},"memory":"y".repeat(100000)});
	super::bound_snapshot(&mut pinned, 2048).unwrap();
	assert_eq!(pinned["task"]["id"], id);
	assert_eq!(pinned["snapshot_truncated"], true);
	assert!(super::estimated_tokens(&pinned.to_string()) <= 2048);
}

#[test]
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

#[test]
fn registration_and_execution_share_the_context_reserve_at_its_boundary() {
	let instructions = "Use the private reference documents.";
	let specifications = vec![json!({"name":"workspace_read"})];
	let window = 8192;
	let output = (window / 8).clamp(256, 4096);
	let fixed = estimated_tokens(&serde_json::to_string(instructions).unwrap())
		.max(serde_json::to_string(instructions).unwrap().len())
		+ estimated_tokens(&serde_json::to_string(&specifications).unwrap())
			.max(serde_json::to_string(&specifications).unwrap().len())
		+ output
		+ REQUEST_FRAMING_RESERVE
		+ MIN_CONTEXT_RESERVE;
	let private_bytes = window - fixed;
	let wrapper = r#"{"reference_documents":""}"#;
	let documents = "x".repeat(private_bytes - wrapper.len());
	let private_context = json!({"reference_documents":documents});
	let registration_budget =
		request_context_budget(window, instructions, &specifications, &private_context).unwrap();
	let execution_budget =
		request_context_budget(window, instructions, &specifications, &private_context).unwrap();
	assert_eq!(registration_budget, MIN_CONTEXT_RESERVE);
	assert_eq!(execution_budget, registration_budget);
	assert!(
		request_context_budget(window - 8, instructions, &specifications, &private_context,)
			.is_err()
	);
}

#[test]
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
