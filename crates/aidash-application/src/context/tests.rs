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
	let mut context = journaled(history());
	context.history[1].event = tool("obsolete", &"日".repeat(4000));
	let asker = FakeJev::new(drop_all);
	let pinned = json!({"task":"航空会社の新規事業計画"});
	compact(&mut context, &asker, 12000, &pinned, "")
		.await
		.unwrap();
	let request = RequestBudget {
		window: 12000,
		instructions: "",
		tools: &[],
		max_output_tokens: 256,
		projection: &RequestProjection::Legacy,
	}
	.request(&context, &pinned);
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
	let mut context = journaled(history());
	context.history[1].event = tool("obsolete", &"old".repeat(6000));
	legacy(&mut context, &FakeJev::new(drop_all), &budget, &pinned)
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
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
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
	let context = journaled(vec![tool("previous", "result")]);
	let event = tool(
		"read",
		&json!({"content":"quotes \" and backslashes \\ and 日本語"}).to_string(),
	);
	let mut after = context.clone();
	after.push(event.clone());
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
		let mut context = journaled(vec![tool("first", text)]);
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
		legacy(&mut context, &asker, &budget, &pinned)
			.await
			.unwrap();
		check_request(budget.window, &budget.request(&context, &pinned)).unwrap();
		let before = json!(context);
		budget.window -= 1;
		assert!(check_request(budget.window, &request).is_err());
		assert!(
			legacy(&mut context, &asker, &budget, &pinned)
				.await
				.is_err()
		);
		assert_eq!(json!(context), before);
		assert!(asker.seen.lock().unwrap().is_empty());
	}
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_uses_jev_without_rewriting_text() {
	let mut context = journaled(history());
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
	assert!(context.execution_summary.is_none());
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
		let mut context = journaled(history());
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
	let mut context = journaled(vec![tool("only", "small")]);
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
	let events = entries(events);
	let output = compaction::prune(&events, &json!({}), &asker, &compaction::Options::default())
		.await
		.unwrap();
	assert_eq!(output.calls_dropped, 1);
	assert_eq!(output.results_truncated, 1);
	assert_eq!(output.history.len(), events.len() - 1);
	assert_eq!(output.history[0], events[0]);
	assert_eq!(
		json!(output.history[1].event)["call"],
		json!(events[2].event)["call"]
	);
	assert!(
		json!(output.history[1].event)["result"]
			.as_str()
			.unwrap()
			.starts_with(&"日".repeat(300))
	);
	assert!(
		json!(output.history[1].event)["result"]
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
	let events = entries(events);
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
	let events = entries(events);
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
	legacy(
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

/// Prune-only behavior of an Agent version without a Context Policy.
async fn legacy(
	context: &mut Context,
	asker: &dyn JevAsker,
	budget: &RequestBudget<'_>,
	pinned: &Value,
) -> Result<()> {
	let policy = policy::Effective::of(None);
	let fitting = Fitting {
		budget,
		pinned,
		policy: &policy,
	};
	match super::compact(context, asker, &fitting).await? {
		Compaction::Fits => Ok(()),
		Compaction::NeedsSummary(_) => panic!("prune-only policy has no Summary Stage"),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn compaction_counts_private_documents_without_disclosing_them() {
	let mut context = journaled(history());
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
	legacy(&mut context, &asker, &budget, &pinned)
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
fn journaled(events: Vec<ContextEvent>) -> Context {
	let mut context = Context::default();
	for event in events {
		context.push(event);
	}
	context
}
fn entries(events: Vec<ContextEvent>) -> Vec<HistoryEntry> {
	journaled(events).history
}
fn check_request(window: usize, request: &provider::ModelRequest) -> aidash_domain::Result<()> {
	request.ensure_fits(window)
}

fn keep_all(_: &str) -> Value {
	json!({"noul":1.0})
}

fn recovery_policy(summary: bool) -> policy::Effective {
	let mut value = json!({"version":"context-recovery/1"});
	if summary {
		value["summary"] = json!({"model":{"id":"summarizer","version":"1.0.0"},"max_tokens":1024});
	}
	let policy: policy::ContextPolicy = serde_json::from_value(value).unwrap();
	policy.validate().unwrap();
	policy::Effective::of(Some(&policy))
}

fn long_history() -> Context {
	let mut events = vec![ContextEvent::Human {
		request: "Never deploy on Fridays".into(),
		request_kind: "INFORMATION_REQUEST".into(),
		response: json!("acknowledged"),
	}];
	events.extend((0..30).map(|i| tool(&format!("old-{i}"), &"x".repeat(400))));
	events.extend((0..6).map(|i| tool(&format!("recent-{i}"), "recent")));
	let mut context = journaled(events);
	context.journal.inferred_through = context.journal.head;
	context
}

fn summarizer() -> summary::SummaryProvider {
	summary::SummaryProvider {
		model: aidash_domain::registry::EntityRef {
			id: "summarizer".into(),
			version: "1.0.0".into(),
		},
		definition_digest: "digest".into(),
	}
}

fn summary_text(constraints: &[&str], unresolved: &[&str], resolved: &[&str]) -> String {
	json!({
		"goal":"Ship the release",
		"constraints":constraints.iter().map(|id| json!({"id":id,"text":"Never deploy on Fridays"})).collect::<Vec<_>>(),
		"decisions":["read every old file"],
		"unresolved":unresolved.iter().map(|id| json!({"id":id,"text":"fix the flaky test"})).collect::<Vec<_>>(),
		"resolved":resolved.iter().map(|id| json!({"id":id,"resolved_by":"old-b0"})).collect::<Vec<_>>(),
		"artifacts":[],
		"verification":[]
	})
	.to_string()
}

struct Scenario {
	pinned: Value,
	window: usize,
}

impl Scenario {
	fn new() -> Self {
		let pinned = json!({"task":{"description":"Ship the release","notes":"n".repeat(20_000)},"reference_documents":"PRIVATE-REFERENCE-123"});
		// Room for the protected events and a summary, not for the 30 old results.
		let mut retained = long_history();
		retained
			.history
			.retain(|entry| !json!(entry.event).to_string().contains("old-"));
		let window = Self::budget(usize::MAX)
			.request(&retained, &pinned)
			.estimated_total_tokens()
			+ 2_000;
		Self { pinned, window }
	}
	fn budget(window: usize) -> RequestBudget<'static> {
		RequestBudget {
			window,
			instructions: "Work on the task",
			tools: &[],
			max_output_tokens: 256,
			projection: &RequestProjection::Legacy,
		}
	}
	async fn plan(&self, context: &mut Context, policy: &policy::Effective) -> Result<Compaction> {
		let budget = Self::budget(self.window);
		let fitting = Fitting {
			budget: &budget,
			pinned: &self.pinned,
			policy,
		};
		super::compact(context, &FakeJev::new(keep_all), &fitting).await
	}
	fn adopt(
		&self,
		plan: &SummaryPlan,
		text: &str,
		policy: &policy::Effective,
	) -> std::result::Result<(Context, usize), Unadopted> {
		let budget = Self::budget(self.window);
		let fitting = Fitting {
			budget: &budget,
			pinned: &self.pinned,
			policy,
		};
		summary_candidate(plan, text, Default::default(), summarizer(), 1024, &fitting)
	}
}

fn needs_summary(compaction: Compaction) -> SummaryPlan {
	match compaction {
		Compaction::NeedsSummary(plan) => *plan,
		Compaction::Fits => panic!("expected the Summary Stage"),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn summary_recovers_history_that_pruning_alone_cannot_fit_across_compactions() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let mut context = long_history();
	let saved = json!(context);
	let plan = needs_summary(scenario.plan(&mut context, &policy).await.unwrap());
	// Planning never changes the saved projection.
	assert_eq!(json!(context), saved);
	assert_eq!(plan.absorbed.len(), 30);
	assert!(
		plan.absorbed
			.iter()
			.all(|entry| json!(entry.event).to_string().contains("old-"))
	);

	let request = summary_request(&plan, &scenario.pinned, 1024);
	assert!(request.tools.is_empty());
	assert!(request.response_format.is_some());
	let provider::ModelContext::Legacy(context) = &request.context else {
		panic!("the summary request is a single JSON message");
	};
	assert!(!context.to_string().contains("PRIVATE-REFERENCE-123"));
	assert_eq!(context["previous_summary"], Value::Null);

	let (first, _) = scenario
		.adopt(&plan, &summary_text(&["c1"], &["u1"], &[]), &policy)
		.unwrap();
	let summary = first.execution_summary.as_ref().unwrap();
	assert_eq!(
		(summary.source.from_seq, summary.source.through_seq),
		(2, 31)
	);
	assert_eq!(summary.policy_version, "context-recovery/1");
	// The human correction and the recent tail stay verbatim.
	assert_eq!(first.history.len(), 7);
	assert!(matches!(first.history[0].event, ContextEvent::Human { .. }));
	assert_eq!(
		first.history[1..].iter().map(|e| e.seq).collect::<Vec<_>>(),
		(32..=37).collect::<Vec<_>>()
	);
	assert!(
		scenario
			.plan(&mut first.clone(), &policy)
			.await
			.is_ok_and(|c| matches!(c, Compaction::Fits))
	);

	// A second compaction merges into the first summary instead of restarting.
	let mut second = first.clone();
	for i in 0..30 {
		second.push(tool(&format!("old-b{i}"), &"y".repeat(400)));
	}
	for i in 0..6 {
		second.push(tool(&format!("recent-b{i}"), "recent"));
	}
	second.journal.inferred_through = second.journal.head;
	let plan = needs_summary(scenario.plan(&mut second, &policy).await.unwrap());
	assert!(plan.absorbed.iter().all(|entry| entry.seq > 31));
	let request = summary_request(&plan, &scenario.pinned, 1024);
	let provider::ModelContext::Legacy(context) = &request.context else {
		panic!("the summary request is a single JSON message");
	};
	assert_eq!(context["previous_summary"]["constraints"][0]["id"], "c1");
	let dropped = scenario
		.adopt(&plan, &summary_text(&[], &["u1"], &[]), &policy)
		.unwrap_err();
	assert_eq!(dropped.outcome, recovery::Outcome::Invalid);
	assert_eq!(
		dropped.rejection,
		Some(summary::Rejection::DroppedItem("c1".into()))
	);
	let (merged, _) = scenario
		.adopt(&plan, &summary_text(&["c1"], &[], &["u1"]), &policy)
		.unwrap();
	let merged_summary = merged.execution_summary.as_ref().unwrap();
	assert_eq!(merged_summary.source.from_seq, 2);
	assert_eq!(
		merged_summary.previous.as_ref().unwrap().digest,
		summary.digest
	);
	assert_eq!(
		merged_summary.content.constraints[0].text,
		"Never deploy on Fridays"
	);
	assert!(merged_summary.content.unresolved.is_empty());
}

#[rstest::rstest]
#[tokio::test]
async fn recent_tail_and_uninferred_events_are_never_absorbed() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let mut context = long_history();
	// Only the first ten old results reached an accepted inference.
	context.journal.inferred_through = 11;
	let plan = needs_summary(scenario.plan(&mut context, &policy).await.unwrap());
	assert_eq!(
		plan.absorbed.iter().map(|e| e.seq).collect::<Vec<_>>(),
		(2..=11).collect::<Vec<_>>()
	);
	let tail: Vec<u64> = context
		.history
		.iter()
		.rev()
		.take(6)
		.map(|e| e.seq)
		.collect();
	for preserve in [6, 12] {
		let candidates = summary_candidates(&long_history(), preserve);
		assert!(candidates.iter().all(|entry| !tail.contains(&entry.seq)));
		assert_eq!(candidates.len(), 36 - preserve);
	}
}

#[rstest::rstest]
#[tokio::test]
async fn invalid_or_insufficient_summaries_are_not_adopted() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let plan = needs_summary(scenario.plan(&mut long_history(), &policy).await.unwrap());
	for text in ["", "not json", &summary_text(&["c1", "c1"], &[], &[])] {
		let unadopted = scenario.adopt(&plan, text, &policy).unwrap_err();
		assert_eq!(unadopted.outcome, recovery::Outcome::Invalid);
	}
	// Smaller than the protected events plus any summary.
	let tight = Scenario {
		window: scenario.window - 3_000,
		..Scenario::new()
	};
	let unadopted = tight
		.adopt(&plan, &summary_text(&["c1"], &[], &[]), &policy)
		.unwrap_err();
	assert_eq!(unadopted.outcome, recovery::Outcome::Insufficient);
}

#[rstest::rstest]
#[tokio::test]
async fn unreducible_context_pauses_with_typed_reasons() {
	let scenario = Scenario::new();
	for policy in [policy::Effective::of(None), recovery_policy(false)] {
		let mut context = long_history();
		let saved = json!(context);
		assert!(matches!(
			scenario.plan(&mut context, &policy).await,
			Err(Error::Context(recovery::Failure::ContextUnreducible))
		));
		assert_eq!(json!(context), saved);
	}
	// Under an explicit policy an unavailable Jev is a typed pause, and the
	// Summary Stage never stands in for it.
	let budget = Scenario::budget(scenario.window);
	let policy = recovery_policy(true);
	let fitting = Fitting {
		budget: &budget,
		pinned: &scenario.pinned,
		policy: &policy,
	};
	assert!(matches!(
		super::compact(
			&mut long_history(),
			&FakeJev::new(|_| json!({"noul":"invalid"})),
			&fitting
		)
		.await,
		Err(Error::Context(recovery::Failure::PruneUnavailable))
	));
	// Legacy versions keep their original error class.
	let legacy_policy = policy::Effective::of(None);
	let fitting = Fitting {
		policy: &legacy_policy,
		..fitting
	};
	assert!(matches!(
		super::compact(
			&mut long_history(),
			&FakeJev::new(|_| json!({"noul":"invalid"})),
			&fitting
		)
		.await,
		Err(Error::External(_))
	));
}

#[rstest::rstest]
#[tokio::test]
async fn revoked_summary_restores_original_journal_events() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let original = long_history();
	let plan = needs_summary(scenario.plan(&mut original.clone(), &policy).await.unwrap());
	let (mut adopted, _) = scenario
		.adopt(&plan, &summary_text(&["c1"], &[], &[]), &policy)
		.unwrap();
	restore_summarized(&mut adopted, original.history.clone()).unwrap();
	assert!(adopted.execution_summary.is_none());
	assert_eq!(adopted.history, original.history);
}

#[rstest::rstest]
#[tokio::test]
async fn revoked_summary_restores_only_the_entries_it_absorbed() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let original = long_history();
	let mut plan = needs_summary(scenario.plan(&mut original.clone(), &policy).await.unwrap());
	// Jev pruned seq 10 before the merge, so the summary never absorbed it.
	plan.absorbed.retain(|entry| entry.seq != 10);
	plan.pruned.history.retain(|entry| entry.seq != 10);
	let (mut adopted, _) = scenario
		.adopt(&plan, &summary_text(&["c1"], &[], &[]), &policy)
		.unwrap();
	let summary = adopted.execution_summary.as_ref().unwrap();
	assert_eq!(summary.source.absorbed, vec![[2, 9], [11, 31]]);
	restore_summarized(&mut adopted, original.history.clone()).unwrap();
	let expected = original
		.history
		.iter()
		.filter(|entry| entry.seq != 10)
		.cloned()
		.collect::<Vec<_>>();
	assert_eq!(adopted.history, expected);
}

#[rstest::rstest]
#[tokio::test]
async fn a_resolution_without_absorbed_evidence_keeps_the_item() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let plan = needs_summary(scenario.plan(&mut long_history(), &policy).await.unwrap());
	let (first, _) = scenario
		.adopt(&plan, &summary_text(&["c1"], &["u1"], &[]), &policy)
		.unwrap();
	let mut second = first;
	for i in 0..30 {
		second.push(tool(&format!("old-b{i}"), &"y".repeat(400)));
	}
	for i in 0..6 {
		second.push(tool(&format!("recent-b{i}"), "recent"));
	}
	second.journal.inferred_through = second.journal.head;
	let plan = needs_summary(scenario.plan(&mut second, &policy).await.unwrap());
	// The recent tail is never absorbed, so it cannot prove a resolution.
	let unproven = summary_text(&["c1"], &[], &["u1"]).replace("old-b0", "recent-b5");
	let unadopted = scenario.adopt(&plan, &unproven, &policy).unwrap_err();
	assert_eq!(
		unadopted.rejection,
		Some(summary::Rejection::UnprovenResolution("u1".into()))
	);
}
