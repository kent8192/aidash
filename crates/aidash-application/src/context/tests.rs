use super::*;
use aidash_domain::context::*;
use aidash_domain::provider;

mod native;

fn tool(id: &str, result: &str) -> ContextEvent {
	event(
		json!({"kind":"tool","call":{"id":id,"name":"read","arguments":{"path":id}},"result":result}),
	)
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
		cache_breakpoints: false,
		disable_provider_transforms: false,
		estimate: Default::default(),
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
		estimate: Default::default(),
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
		estimate: Default::default(),
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
			estimate: Default::default(),
		};
		let request = budget.request(&context, &pinned);
		budget.window = request.estimated_total_tokens();
		prune_only(&mut context, &budget, &pinned).await.unwrap();
		check_request(budget.window, &budget.request(&context, &pinned)).unwrap();
		let before = json!(context);
		budget.window -= 1;
		assert!(check_request(budget.window, &request).is_err());
		assert!(matches!(
			prune_only(&mut context, &budget, &pinned).await,
			Err(Error::Context(recovery::Failure::ContextUnreducible))
		));
		assert_eq!(json!(context), before);
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
		estimate: Default::default(),
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
		Default::default(),
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
			&RequestProjection::Legacy,
			Default::default(),
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

/// An Agent version without a Context Policy and without a compaction
/// Decider: its context either fits or is unreducible.
async fn prune_only(
	context: &mut Context,
	budget: &RequestBudget<'_>,
	pinned: &Value,
) -> Result<()> {
	let policy = policy::Effective::of(None);
	let fitting = Fitting {
		budget,
		pinned,
		policy: &policy,
	};
	match super::compact_decided(context, None, &fitting).await? {
		Compaction::Fits => Ok(()),
		Compaction::NeedsSummary(_) | Compaction::Saved => {
			panic!("a prune-only policy without a Decider never summarizes or saves")
		}
	}
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
fn check_request(window: usize, request: &provider::ModelRequest) -> aidash_domain::Result<()> {
	request.ensure_fits(window)
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
			estimate: Default::default(),
		}
	}
	async fn plan(&self, context: &mut Context, policy: &policy::Effective) -> Result<Compaction> {
		let budget = Self::budget(self.window);
		let fitting = Fitting {
			budget: &budget,
			pinned: &self.pinned,
			policy,
		};
		super::compact_decided(context, None, &fitting).await
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
		Compaction::Fits | Compaction::Saved => panic!("expected the Summary Stage"),
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
	let dependencies = adopted
		.execution_summary
		.as_ref()
		.unwrap()
		.dependencies
		.clone();
	restore_summarized(&mut adopted, original.history.clone()).unwrap();
	assert!(adopted.execution_summary.is_none());
	assert_eq!(adopted.history, original.history);
	// The restored originals stay guarded by the revoked summary's sources.
	assert_eq!(adopted.revoked_summary_dependencies, Some(dependencies));
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
