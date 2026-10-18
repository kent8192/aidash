//! The Harness handles Web research only through declared behavior: citation
//! checks follow `yields: Citations` and disclosure waits follow the generic
//! approval result. No research tool name is consulted.
use super::*;
use aidash_domain::research::{
	Operation,
	citations::{self, EvidenceRef},
};

const KNOWN: EvidenceRef = EvidenceRef(Uuid::from_u128(0xe1));
const UNKNOWN: EvidenceRef = EvidenceRef(Uuid::from_u128(0xe2));

fn bind(fixture: &mut Fixture, alias: &str, tool: Arc<Scripted>) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert(alias.into(), tool);
	state.known_evidence = vec![KNOWN];
}

fn answer(run: &mut Run, text: String) {
	run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text,
			usage_complete: true,
			..Default::default()
		},
		..Default::default()
	}));
}

fn checks(fixture: &Fixture) -> usize {
	fixture
		.backend
		.0
		.calls
		.lock()
		.unwrap()
		.iter()
		.filter(|&&call| call == "store.evidence")
		.count()
}

#[rstest]
#[tokio::test]
async fn delivered_evidence_citations_publish_the_answer_unchanged(mut fixture: Fixture) {
	bind(&mut fixture, "lookup", Scripted::research(Operation::Find));
	let text = format!("The page says so [[cite:{KNOWN}]].");
	answer(&mut fixture.run, text.clone());
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(*fixture.backend.0.artifacts.lock().unwrap(), [json!(text)]);
	assert_eq!(checks(&fixture), 1);
}

#[rstest]
#[tokio::test]
async fn invalid_citations_get_one_correction_and_never_publish_a_fabricated_reference(
	mut fixture: Fixture,
) {
	// The citing operation has a provider-neutral alias chosen by the Agent.
	bind(
		&mut fixture,
		"evidence_reader",
		Scripted::research(Operation::Fetch),
	);
	answer(
		&mut fixture.run,
		format!("Claim [[cite:{UNKNOWN}]] and [[cite:{KNOWN}]]."),
	);
	advance_sources(&mut fixture).await.unwrap();
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.citation_correction_required");
	assert_eq!(saved.phase(), RunPhase::Thinking);
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::CitationCorrectionRequired { invalid, instruction, .. })
			if *invalid == [UNKNOWN.to_string()]
				&& instruction == citations::CORRECTION_INSTRUCTION
	));
	assert!(fixture.backend.0.artifacts.lock().unwrap().is_empty());

	// A second invalid answer is withheld rather than published.
	answer(&mut fixture.run, format!("Still [[cite:{UNKNOWN}]]."));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(
		*fixture.backend.0.artifacts.lock().unwrap(),
		[json!(citations::WITHHELD_ANSWER)]
	);
}

#[rstest]
#[tokio::test]
async fn invalid_citations_at_the_step_limit_are_withheld_without_a_correction(
	mut fixture: Fixture,
) {
	bind(&mut fixture, "lookup", Scripted::research(Operation::Find));
	// The fixture Agent allows 64 steps; no inference step remains for a correction.
	fixture.run.step = 63;
	answer(&mut fixture.run, format!("Claim [[cite:{UNKNOWN}]]."));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(
		*fixture.backend.0.artifacts.lock().unwrap(),
		[json!(citations::WITHHELD_ANSWER)]
	);
}

#[rstest]
#[tokio::test]
async fn malformed_markers_are_corrected_like_unknown_evidence(mut fixture: Fixture) {
	bind(&mut fixture, "lookup", Scripted::research(Operation::Find));
	answer(&mut fixture.run, "Claim [[cite:ev_invented]].".into());
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(last_write(&fixture).0, "run.citation_correction_required");
}

#[rstest]
#[tokio::test]
async fn runs_without_citing_operations_are_not_verified(mut fixture: Fixture) {
	// Search yields Source records only; snippets are never citable evidence.
	bind(
		&mut fixture,
		"lookup",
		Scripted::research(Operation::Search),
	);
	let text = format!("Unverified [[cite:{UNKNOWN}]].");
	answer(&mut fixture.run, text.clone());
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(*fixture.backend.0.artifacts.lock().unwrap(), [json!(text)]);
	assert_eq!(checks(&fixture), 0);
}

#[rstest]
#[tokio::test]
async fn disclosure_approval_waits_like_every_capability_approval(mut fixture: Fixture) {
	let fetch = Scripted::research(Operation::Fetch);
	let approval = Uuid::from_u128(0xa1);
	*fetch.outputs.lock().unwrap() = [json!({
		"version": 1,
		"operation": "fetch",
		"status": "approval_required",
		"approval_id": approval,
		"instruction": "repeat after approval",
	})]
	.into();
	bind(&mut fixture, "page", fetch.clone());
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"page",
			json!({"url":"https://example.com/"}),
		)],
	);
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fetch.invocations(), 1);
	let (_, saved) = last_write(&fixture);
	assert!(matches!(
		&saved.state,
		RunState::Waiting(waiting)
			if matches!(**waiting, WaitingState::CoreApproval { approval_id, .. } if approval_id == approval)
	));
}

#[rstest]
fn research_contracts_declare_yields_and_disclosure_per_operation() {
	use aidash_domain::tool::{DisclosureBoundary, providers::research_contract};
	let search = research_contract(Operation::Search);
	let fetch = research_contract(Operation::Fetch);
	let find = research_contract(Operation::Find);
	assert!(search.behavior.yields(Yield::Search) && !search.behavior.yields(Yield::Citations));
	assert!(fetch.behavior.yields(Yield::Fetch) && fetch.behavior.yields(Yield::Citations));
	assert!(find.behavior.yields(Yield::Citations));
	assert!(search.behavior.disclosure_approval && fetch.behavior.disclosure_approval);
	assert!(!find.behavior.disclosure_approval);
	assert_eq!(find.disclosure, DisclosureBoundary::Local);
	assert!(
		[search, fetch, find]
			.iter()
			.all(|contract| contract.replay_safe() && !contract.remote_exposure)
	);
}

#[rstest]
#[tokio::test]
async fn text_beside_tool_calls_is_not_published_with_invalid_citations(mut fixture: Fixture) {
	let find = Scripted::research(Operation::Find);
	*find.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	bind(&mut fixture, "lookup", find);
	fixture.run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text: format!("Interim [[cite:{UNKNOWN}]]"),
			tool_calls: vec![call("call-0", "lookup", json!({}))],
			usage_complete: true,
			..Default::default()
		},
		..Default::default()
	}));
	advance_sources(&mut fixture).await.unwrap();
	let calls = fixture.backend.0.calls.lock().unwrap();
	assert!(!calls.contains(&"home.output"), "{calls:?}");
	assert!(calls.contains(&"store.evidence"));
}

/// Context inspection shows the whole journal, so withheld text never
/// enters a Native Model Turn either; the turn keeps only its calls.
#[rstest]
#[tokio::test]
async fn a_native_model_turn_records_no_text_the_citation_gate_withheld(mut fixture: Fixture) {
	// Arrange
	Arc::get_mut(&mut fixture.backend.0).unwrap().projection = ProjectionVersion::Native;
	let find = Scripted::research(Operation::Find);
	*find.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	bind(&mut fixture, "lookup", find);
	fixture.run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text: format!("Interim [[cite:{UNKNOWN}]]"),
			tool_calls: vec![call("call-0", "lookup", json!({}))],
			usage_complete: true,
			..Default::default()
		},
		response_epoch: 4,
		..Default::default()
	}));
	// Act
	advance_sources(&mut fixture).await.unwrap();
	// Assert
	let calls = fixture.backend.0.calls.lock().unwrap().clone();
	assert!(!calls.contains(&"home.output"), "{calls:?}");
	assert_eq!(
		fixture.run.context.events().next(),
		Some(&ContextEvent::Assistant {
			epoch: 4,
			text: String::new(),
			calls: vec!["call-0".into()],
		})
	);
}

#[rstest]
#[tokio::test]
async fn each_user_input_gets_its_own_correction(mut fixture: Fixture) {
	bind(&mut fixture, "lookup", Scripted::research(Operation::Find));
	answer(&mut fixture.run, format!("Claim [[cite:{UNKNOWN}]]."));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(last_write(&fixture).0, "run.citation_correction_required");
	// New user input arrives; the next invalid answer is corrected again.
	fixture.run.observed_input_seq += 1;
	answer(&mut fixture.run, format!("Again [[cite:{UNKNOWN}]]."));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(last_write(&fixture).0, "run.citation_correction_required");
	assert!(fixture.backend.0.artifacts.lock().unwrap().is_empty());
}

fn research_result(text: &str) -> Value {
	json!({
		"contract": "aidash.research/1",
		"version": 1,
		"operation": "fetch",
		"status": "ok",
		"limits": {"max_bytes": 32768, "truncated": false},
		"data": {"text": text},
	})
}

fn research_history(fixture: &mut Fixture) {
	// The alias is bound to nothing: a result is recognized by its envelope,
	// so a renamed, unloaded or unbound research tool is redacted all the same.
	fixture.run.context.push(ContextEvent::tool(
		call(
			"call-page",
			"renamed_page",
			json!({"url": "https://example.com/"}),
		),
		research_result("a revoked passage"),
	));
	fixture.run.context.push(ContextEvent::tool(
		call("call-other", "unbound", json!({})),
		json!("an unrelated result"),
	));
	fixture.run.context.journal.inferred_through = fixture.run.context.journal.head;
}

fn first_request(fixture: &Fixture) -> String {
	format!("{:?}", fixture.backend.0.requests.lock().unwrap()[0])
}

fn revoke(fixture: &mut Fixture) {
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.research_revoked = true;
}

#[rstest]
#[tokio::test]
async fn revoked_research_results_never_reach_a_model_request_again(fixture: Fixture) {
	let mut fixture = thinking(fixture);
	revoke(&mut fixture);
	research_history(&mut fixture);
	let _ = advance_sources(&mut fixture).await;
	let request = first_request(&fixture);
	assert!(!request.contains("a revoked passage"), "{request}");
	assert!(
		request.contains("Research evidence was revoked"),
		"{request}"
	);
	// Results that are not Research evidence are untouched.
	assert!(request.contains("an unrelated result"), "{request}");
	// The call stays paired with its placeholder.
	assert!(fixture.run.context.events().any(|event| matches!(
		event,
		ContextEvent::Tool { call, result }
			if call.id == "call-page" && ContextEvent::is_revoked_result(result)
	)));
	assert!(fixture.run.context.research_revoked);
}

#[rstest]
#[tokio::test]
async fn a_revocation_committed_while_the_request_is_built_never_reaches_the_provider(
	fixture: Fixture,
) {
	// Arrange: the revocation lands after the check at the start of the step.
	let mut fixture = thinking(fixture);
	research_history(&mut fixture);
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.research_revoked_after = Some(1);

	// Act
	let _ = advance_sources(&mut fixture).await;
	let sent = fixture.backend.0.requests.lock().unwrap().len();
	let _ = advance_sources(&mut fixture).await;

	// Assert: nothing went out in the step that saw the revocation late; the
	// next step was built from the redacted projection.
	assert_eq!(sent, 0);
	assert!(fixture.run.context.research_revoked);
	let request = first_request(&fixture);
	assert!(!request.contains("a revoked passage"), "{request}");
	assert!(
		request.contains("Research evidence was revoked"),
		"{request}"
	);
	assert!(fixture.run.context.research_revoked);
}

#[rstest]
#[tokio::test]
async fn research_results_of_a_run_not_revoked_are_sent_unchanged(fixture: Fixture) {
	let mut fixture = thinking(fixture);
	research_history(&mut fixture);
	let _ = advance_sources(&mut fixture).await;
	assert!(first_request(&fixture).contains("a revoked passage"));
	assert!(!fixture.run.context.research_revoked);
}

#[rstest]
#[tokio::test]
async fn runs_without_research_results_or_a_summary_never_ask_whether_evidence_was_revoked(
	fixture: Fixture,
) {
	let mut fixture = thinking(fixture);
	let _ = advance_sources(&mut fixture).await;
	assert!(
		!fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"store.research_revoked")
	);
}

#[rstest]
#[tokio::test]
async fn a_summary_that_absorbed_revoked_research_is_discarded_without_pausing(
	mut fixture: Fixture,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.context_policy = Some(recovery_policy(true));
	state.jev_retention = Some(1.0);
	*state.summary_text.lock().unwrap() = Some(valid_summary());
	thinking_with_history(&mut fixture, 40);
	let research = &mut fixture.run.context.history[1];
	let seq = research.seq;
	let ContextEvent::Tool { result, .. } = &mut research.event else {
		unreachable!()
	};
	*result = research_result(&format!("a revoked passage {}", "y".repeat(4000)));
	advance_sources(&mut fixture).await.unwrap();
	let summary = fixture.run.context.execution_summary.as_ref().unwrap();
	assert!(
		summary.source.absorbs(seq),
		"the summary absorbed the result"
	);

	revoke(&mut fixture);
	let requests = fixture.backend.0.requests.lock().unwrap().len();
	advance_sources(&mut fixture).await.unwrap();
	// The Run continues: nothing waits for revoked evidence to become
	// readable again, which it never does.
	assert!(!events(&fixture).contains(&"context.summary_revoked".to_owned()));
	assert!(fixture.run.context.research_revoked);
	let sent = fixture.backend.0.requests.lock().unwrap()[requests..]
		.iter()
		.map(|request| format!("{request:?}"))
		.collect::<Vec<_>>();
	assert!(!sent.is_empty());
	for request in &sent {
		assert!(!request.contains("a revoked passage"), "{request}");
	}
	// The discarded summary's range was restored and sent again, with the
	// placeholder where the research result was.
	assert!(
		sent.iter()
			.any(|request| request.contains("Research evidence was revoked")),
		"{sent:?}"
	);
	// The restored range carries the placeholder wherever it is still projected.
	assert!(fixture.run.context.history.iter().all(|entry| {
		entry.seq != seq
			|| matches!(&entry.event, ContextEvent::Tool { result, .. } if ContextEvent::is_revoked_result(result))
	}));
}

fn cut(head: &str) -> Value {
	json!(format!(
		"{head}\n[fast-jev-compaction truncated 2000 chars of this tool result; full result remains in the execution journal]"
	))
}

#[rstest]
#[tokio::test]
async fn a_research_result_compaction_cut_to_a_string_is_redacted_from_its_journaled_original(
	fixture: Fixture,
) {
	let mut fixture = thinking(fixture);
	revoke(&mut fixture);
	let originals = [
		("call-page", research_result("a revoked passage")),
		("call-note", json!("a workspace note")),
	];
	for (id, original) in originals {
		let head = format!("{{\"data\":{original}");
		fixture.run.context.push(ContextEvent::tool(
			call(id, "renamed", json!({})),
			cut(&head),
		));
		let seq = fixture.run.context.history.last().unwrap().seq;
		fixture.backend.0.journal.lock().unwrap().push((
			fixture.run.id,
			aidash_domain::context::HistoryEntry {
				seq,
				event: ContextEvent::tool(call(id, "renamed", json!({})), original),
			},
		));
	}
	fixture.run.context.journal.inferred_through = fixture.run.context.journal.head;
	let _ = advance_sources(&mut fixture).await;
	let request = first_request(&fixture);
	assert!(!request.contains("a revoked passage"), "{request}");
	assert!(
		request.contains("Research evidence was revoked"),
		"{request}"
	);
	// A cut result whose original was no research result is kept.
	assert!(request.contains("a workspace note"), "{request}");
}

#[rstest]
#[tokio::test]
async fn an_elided_research_result_loses_the_arguments_its_original_named(fixture: Fixture) {
	// Arrange: compaction elided two results whole, keeping their calls; the
	// journal holds the originals.
	let mut fixture = thinking(fixture);
	revoke(&mut fixture);
	let originals = [
		(
			"call-search",
			json!({"query": "a revoked query"}),
			research_result("a revoked passage"),
		),
		(
			"call-note",
			json!({"note": "a kept note"}),
			json!("a workspace note"),
		),
	];
	for (id, arguments, original) in originals {
		fixture.run.context.push(ContextEvent::tool(
			call(id, "renamed", arguments.clone()),
			ContextEvent::elided_result(id),
		));
		let seq = fixture.run.context.history.last().unwrap().seq;
		fixture.backend.0.journal.lock().unwrap().push((
			fixture.run.id,
			aidash_domain::context::HistoryEntry {
				seq,
				event: ContextEvent::tool(call(id, "renamed", arguments), original),
			},
		));
	}
	fixture.run.context.journal.inferred_through = fixture.run.context.journal.head;

	// Act
	let _ = advance_sources(&mut fixture).await;

	// Assert: the research call no longer names its query; the other keeps
	// its arguments.
	let request = first_request(&fixture);
	assert!(!request.contains("a revoked query"), "{request}");
	assert!(
		request.contains("Research evidence was revoked"),
		"{request}"
	);
	assert!(request.contains("a kept note"), "{request}");
}
