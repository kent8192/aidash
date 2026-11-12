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
		provider_tools: vec![],
		instructions: String::new(),
		context: json!({}).into(),
		tools: vec![],
		max_output_tokens: 4096,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
		routing_run: None,
		disable_provider_transforms: false,
		input_format: Default::default(),
		estimate: Default::default(),
	};
	assert!(check_request(2000, &request).is_err());
}

#[rstest::rstest]
fn request_check_reserves_the_registered_model_maximum_with_input_and_framing() {
	let budget = RequestBudget {
		input_format: Default::default(),
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
		input_format: Default::default(),
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
			input_format: Default::default(),
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
#[case::chat(provider::InputFormat::Chat)]
#[case::responses(provider::InputFormat::Responses)]
fn registration_and_execution_share_the_context_reserve_at_its_boundary(
	#[case] input_format: provider::InputFormat,
) {
	let instructions = "Use the private reference documents.";
	let specifications = vec![];
	let output = 4096;
	let private_context = json!({"reference_documents":"private \"quoted\" reference"});
	let request = RequestBudget {
		input_format,
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
		input_format,
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
			input_format,
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

/// Check fitting with the default recovery policy.
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
	match super::compact(context, &fitting, 0)? {
		Compaction::Fits => Ok(()),
		Compaction::Offloaded {
			context: candidate, ..
		} => {
			*context = *candidate;
			Ok(())
		}
		Compaction::NeedsSummary(_, _) => {
			Err(Error::Context(recovery::Failure::ContextUnreducible))
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
		value["summary"] = json!({"max_tokens":1024});
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

fn summary_text(_constraints: &[&str], _unresolved: &[&str], _resolved: &[&str]) -> String {
	json!({"retained":[], "labels":[], "prose":"read every old file"}).to_string()
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
			input_format: Default::default(),
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
		if fitting.fits(context) {
			return Ok(Compaction::Fits);
		}
		super::summary_plan(context.clone(), &fitting)
			.map(|plan| Compaction::NeedsSummary(plan, Default::default()))
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
		Compaction::NeedsSummary(plan, _) => *plan,
		Compaction::Fits | Compaction::Offloaded { .. } => panic!("expected the Summary Stage"),
	}
}

#[test]
fn repeated_journal_reads_request_a_summary_even_when_request_fits() {
	let mut context = journaled(vec![
		tool("original", "saved evidence"),
		tool("later", "later"),
	]);
	context.journal.inferred_through = context.journal.head;
	let policy = policy::Effective::of(None);
	let range = super::offload::JournalRange {
		from_seq: 1,
		through_seq: 1,
	};
	for result in 3..=5 {
		super::offload::note_journal_read(&mut context, range, result, 10, &policy);
	}
	let budget = Scenario::budget(100_000);
	let pinned = json!({"task":{"description":"Work"}});
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	let Compaction::NeedsSummary(plan, _) = super::compact(&context, &fitting, 10).unwrap() else {
		panic!("repeated read should request the Summary Stage");
	};
	assert!(plan.absorbed.iter().any(|entry| entry.seq == 1));
	let draft =
		json!({"retained":[],"labels":[],"prose":"The original was read repeatedly."}).to_string();
	let (adopted, _) = summary_candidate(
		&plan,
		&draft,
		Default::default(),
		summarizer(),
		1024,
		&fitting,
	)
	.unwrap();
	assert!(!adopted.offload_reads.summary_pending);
}

#[rstest::rstest]
#[tokio::test]
async fn summary_merges_old_history_and_refuses_a_dropped_previous_item() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let mut context = long_history();
	let saved = json!(context);
	let plan = needs_summary(scenario.plan(&mut context, &policy).await.unwrap());
	assert_eq!(json!(context), saved);
	let request = summary_request(&plan, &scenario.pinned, 1024);
	assert!(request.tools.is_empty());
	assert!(
		!json!(request.context)
			.to_string()
			.contains("PRIVATE-REFERENCE-123")
	);
	let (first, _) = scenario
		.adopt(&plan, &summary_text(&[], &[], &[]), &policy)
		.unwrap();
	let previous = first.execution_summary.as_ref().unwrap();
	assert_eq!(previous.content.goal, "Ship the release");
	let mut second = first.clone();
	for i in 0..30 {
		second.push(tool(&format!("old-b{i}"), &"y".repeat(400)));
	}
	for i in 0..6 {
		second.push(tool(&format!("recent-b{i}"), "recent"));
	}
	second.journal.inferred_through = second.journal.head;
	let plan = needs_summary(scenario.plan(&mut second, &policy).await.unwrap());
	let error = scenario
		.adopt(&plan, &summary_text(&[], &[], &[]), &policy)
		.unwrap_err();
	assert!(matches!(
		error.rejection,
		Some(summary::Rejection::DroppedItem(_))
	));
	let draft = json!({"retained":SummaryContent::retained_ids(Some(previous)),"labels":[],"prose":"continued"}).to_string();
	let (merged, _) = scenario.adopt(&plan, &draft, &policy).unwrap();
	assert_eq!(
		merged.execution_summary.unwrap().previous.unwrap().digest,
		previous.digest
	);
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
async fn invalid_summaries_are_rejected_and_reductions_can_continue() {
	let scenario = Scenario::new();
	let policy = recovery_policy(true);
	let plan = needs_summary(scenario.plan(&mut long_history(), &policy).await.unwrap());
	for text in ["", "not json", "{\"retained\":[]}"] {
		let unadopted = scenario.adopt(&plan, text, &policy).unwrap_err();
		assert_eq!(unadopted.outcome, recovery::Outcome::Invalid);
	}
	// Smaller than the protected events plus any summary.
	let tight = Scenario {
		window: scenario.window - 3_000,
		..Scenario::new()
	};
	let (partial, after) = tight
		.adopt(&plan, &summary_text(&["c1"], &[], &[]), &policy)
		.unwrap();
	assert!(after > tight.window);
	assert!(after < plan.before_tokens);
	assert!(partial.execution_summary.is_some());
}

#[rstest::rstest]
#[tokio::test]
async fn unreducible_context_pauses_when_no_events_can_be_summarized() {
	let policy = policy::Effective::of(None);
	let context = Context::default();
	let budget = Scenario::budget(256);
	let pinned = json!({"task":"x".repeat(10_000)});
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	assert!(matches!(
		super::compact(&context, &fitting, 0),
		Err(Error::Context(recovery::Failure::ContextUnreducible))
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
	// Sequence 10 was omitted before the merge and must not be restored.
	plan.absorbed.retain(|entry| entry.seq != 10);
	plan.pruned.history.retain(|entry| entry.seq != 10);
	let (mut adopted, _) = scenario
		.adopt(&plan, &summary_text(&["c1"], &[], &[]), &policy)
		.unwrap();
	let summary = adopted.execution_summary.as_ref().unwrap();
	assert_eq!(summary.source.absorbed, vec![[1, 9], [11, 31]]);
	restore_summarized(&mut adopted, original.history.clone()).unwrap();
	let expected = original
		.history
		.iter()
		.filter(|entry| entry.seq != 10)
		.cloned()
		.collect::<Vec<_>>();
	assert_eq!(adopted.history, expected);
}

#[tokio::test]
async fn repeated_summary_adoption_bounds_older_prose_in_context_bytes() {
	let mut scenario = Scenario::new();
	scenario.pinned["task"]["description"] = json!("g".repeat(433));
	let policy = recovery_policy(true);
	let mut context = long_history();
	for round in 0..4 {
		let plan = needs_summary(scenario.plan(&mut context, &policy).await.unwrap());
		let previous = plan.pruned.execution_summary.as_deref();
		let draft = json!({"retained":SummaryContent::retained_ids(previous),"labels":[],"prose":format!("round {round}: {}", "x".repeat(700))}).to_string();
		let (adopted, _) = scenario.adopt(&plan, &draft, &policy).unwrap();
		let content = &adopted.execution_summary.as_ref().unwrap().content;
		assert!(serde_json::to_string(content).unwrap().len() <= scenario.window / 8);
		assert_eq!(content.goal, "g".repeat(433));
		assert!(content.decisions.iter().any(|item| {
			item.starts_with("Full tool results remain in Context Journal sequences ")
		}));
		assert!(
			content
				.decisions
				.iter()
				.any(|item| item.starts_with(&format!("round {round}:")))
		);
		context = adopted;
		for i in 0..30 {
			context.push(tool(&format!("old-{round}-{i}"), &"z".repeat(400)));
		}
		for i in 0..6 {
			context.push(tool(&format!("recent-{round}-{i}"), "recent"));
		}
		context.journal.inferred_through = context.journal.head;
	}
	let content = &context.execution_summary.as_ref().unwrap().content;
	assert!(
		!content
			.decisions
			.iter()
			.any(|item| item.starts_with("round 0:"))
	);
}

#[rstest::rstest]
fn an_oversized_rehydrated_original_goes_back_to_its_marker_so_the_summary_fits() {
	// Arrange: the oldest absorbed result is larger than the summary window.
	let mut context = journaled(vec![
		tool("huge", &"x".repeat(40_000)),
		tool("small", "ok"),
		tool("recent", "recent"),
	]);
	context.journal.inferred_through = context.journal.head;
	let mut plan = SummaryPlan {
		pruned: context.clone(),
		absorbed: context.history[..2].to_vec(),
		people: vec![],
		before_tokens: usize::MAX,
	};
	let pinned = json!({"task":{"description":"Work"}});
	let window = 8_000;
	assert!(
		summary_request(&plan, &pinned, 512)
			.ensure_fits(window)
			.is_err()
	);

	// Act
	assert!(offload_largest_absorbed(&mut plan.absorbed, 0));

	// Assert: only the large result became its marker; the request fits and
	// the adopted summary points to the original in the Journal.
	assert!(matches!(
		&plan.absorbed[0].event,
		ContextEvent::Tool { result, .. } if super::offload::is_marker(result) && result["journal"]["from_seq"] == 1
	));
	assert!(matches!(
		&plan.absorbed[1].event,
		ContextEvent::Tool { result, .. } if result == "ok"
	));
	assert!(
		summary_request(&plan, &pinned, 512)
			.ensure_fits(window)
			.is_ok()
	);
	assert!(!offload_largest_absorbed(&mut plan.absorbed[1..], 0));
	let policy = recovery_policy(true);
	let budget = Scenario::budget(window);
	let fitting = Fitting {
		budget: &budget,
		pinned: &pinned,
		policy: &policy,
	};
	let draft = json!({"retained":[],"labels":[],"prose":"read the huge file"}).to_string();
	let (adopted, _) = summary_candidate(
		&plan,
		&draft,
		Default::default(),
		summarizer(),
		512,
		&fitting,
	)
	.unwrap();
	let summary = adopted.execution_summary.unwrap();
	assert!(
		summary
			.content
			.decisions
			.iter()
			.any(|decision| decision.contains("sequences 1 through 1"))
	);
}
