//! The Harness handles Web research only through declared behavior: citation
//! checks follow `yields: Citations` and disclosure waits follow the generic
//! approval result. No research tool name is consulted.
use super::*;
use aidash_domain::research::{
	Operation,
	citations::{self, EvidenceRef},
};
use aidash_domain::tool::Yield;

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
async fn an_approval_raised_inside_a_call_resumes_that_call_without_another_response(
	mut fixture: Fixture,
) {
	// Arrange
	let fetch = Scripted::research(Operation::Fetch);
	let approval = Uuid::from_u128(0xa1);
	*fetch.outputs.lock().unwrap() = [
		json!({
			"version": 1,
			"operation": "fetch",
			"status": "approval_required",
			"approval_id": approval,
			"instruction": "the Run waits",
		}),
		json!({"version": 1, "operation": "fetch", "status": "ok"}),
	]
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

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(fetch.invocations(), 1);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_approval");
	let RunState::Waiting(waiting) = &saved.state else {
		panic!("the Run waits for the approval");
	};
	let WaitingState::Approval {
		approval: ApprovalRef::Capability { approval_id },
		resume,
	} = &**waiting
	else {
		panic!("the Run waits for a capability approval");
	};
	assert_eq!(*approval_id, approval);
	assert_eq!((resume.cursor, resume.approval), (0, Some(approval)));
	assert!(
		saved
			.context
			.events()
			.all(|event| !matches!(event, ContextEvent::Tool { .. })),
		"the approval request is not the call's result"
	);

	// The approval is decided: the Run wakes into the same call and makes it.
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	let keys = fetch.keys();
	let run = fixture.run.id;
	assert_eq!(
		keys,
		[
			format!("{run}:0:0"),
			format!("{run}:0:0:approval:{approval}")
		]
	);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if result["status"] == "ok"
	));
	assert!(
		matches!(&saved.state, RunState::ToolCall(state) if state.cursor == 1 && state.approval.is_none())
	);
	assert!(
		!fixture.backend.calls().contains(&"inference.start"),
		"the approved call needs no further model response"
	);
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
			provider_activity: vec![],
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

const EXPIRED_DOCUMENT: &str = "0190f1d2-0000-7000-8000-0000000000d1";

#[rstest]
#[case::already_expired(false)]
#[case::expired_during_context_reads(true)]
#[tokio::test]
async fn expired_provider_results_never_reach_a_model_request_again(
	fixture: Fixture,
	#[case] late: bool,
) {
	// Arrange: a page a provider with account terms read, now expired.
	let mut fixture = thinking(fixture);
	let mut page = research_result("an expired passage");
	page["data"]["document_id"] = json!(EXPIRED_DOCUMENT);
	fixture.run.context.push(ContextEvent::tool(
		call(
			"call-page",
			"renamed_page",
			json!({"url": "https://example.com/sent"}),
		),
		page,
	));
	research_history(&mut fixture);
	let through = chrono::Utc::now();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.expired_research = aidash_domain::research::expiry::ExpiredResults {
		sources: Default::default(),
		documents: [EXPIRED_DOCUMENT.parse().unwrap()].into(),
		through: Some(through),
	};

	if late {
		Arc::get_mut(&mut fixture.backend.0)
			.unwrap()
			.expired_research_after = Some(1);
	}

	// Act
	let _ = advance_sources(&mut fixture).await;

	if late {
		assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
		assert_eq!(
			events(&fixture).last().map(String::as_str),
			Some("run.research_expired")
		);
		advance_sources(&mut fixture).await.unwrap();
	}

	// Assert: only the expired result is replaced; what Aidash sent stays.
	let request = first_request(&fixture);
	assert!(!request.contains("an expired passage"), "{request}");
	assert!(
		request.contains("account terms no longer let this result be kept"),
		"{request}"
	);
	assert!(request.contains("https://example.com/sent"), "{request}");
	assert!(request.contains("a revoked passage"), "{request}");
	assert!(fixture.run.context.events().any(|event| matches!(
		event,
		ContextEvent::Tool { call, result }
			if call.id == "call-page" && ContextEvent::is_expired_result(result)
	)));
	assert_eq!(fixture.run.context.research_expired_through, Some(through));
	assert!(!fixture.run.context.research_revoked);
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

#[rstest]
#[tokio::test]
async fn a_summary_that_absorbed_an_expired_provider_result_is_discarded_without_pausing(
	mut fixture: Fixture,
) {
	// Arrange: a summary absorbs a page a provider with account terms read.
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
	let mut page = research_result(&format!("an expired passage {}", "y".repeat(4000)));
	page["data"]["document_id"] = json!(EXPIRED_DOCUMENT);
	*result = page;
	advance_sources(&mut fixture).await.unwrap();
	assert!(
		fixture
			.run
			.context
			.execution_summary
			.as_ref()
			.unwrap()
			.source
			.absorbs(seq),
		"the summary absorbed the result"
	);

	// Act: the Research account terms no longer keep that page.
	let through = chrono::Utc::now();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.expired_research = aidash_domain::research::expiry::ExpiredResults {
		sources: Default::default(),
		documents: [EXPIRED_DOCUMENT.parse().unwrap()].into(),
		through: Some(through),
	};
	let requests = fixture.backend.0.requests.lock().unwrap().len();
	advance_sources(&mut fixture).await.unwrap();

	// Assert: the Run continues without the summary and without the page.
	assert!(!events(&fixture).contains(&"context.summary_revoked".to_owned()));
	assert_eq!(fixture.run.context.research_expired_through, Some(through));
	let sent = fixture.backend.0.requests.lock().unwrap()[requests..]
		.iter()
		.map(|request| format!("{request:?}"))
		.collect::<Vec<_>>();
	assert!(!sent.is_empty());
	for request in &sent {
		assert!(!request.contains("an expired passage"), "{request}");
	}
	assert!(
		sent.iter()
			.any(|request| request.contains("account terms no longer let this result be kept")),
		"{sent:?}"
	);
	assert!(fixture.run.context.history.iter().all(|entry| {
		entry.seq != seq
			|| matches!(&entry.event, ContextEvent::Tool { result, .. } if ContextEvent::is_expired_result(result))
	}));
}

fn cut(head: &str) -> Value {
	json!(format!(
		"{head}\n[compaction kept only these first 300 of 2300 characters to fit the context window; rely on them and on what you already concluded, and call the tool again only if the rest is still needed verbatim]"
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
				research: None,
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
				research: None,
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

#[rstest]
#[case::fresh(false)]
#[case::completed_replay(true)]
#[tokio::test]
async fn expired_view_results_are_revalidated_before_selecting_model_media(
	mut fixture: Fixture,
	#[case] replay: bool,
) {
	let view = Scripted::research(Operation::View);
	let output = json!({"contract":"aidash.research/1", "version":1, "operation":"view", "status":"ok", "data":{"evidence_ref":KNOWN.to_string(), "image":{"digest":"sha256:fixture"}}});
	*view.outputs.lock().unwrap() = [output.clone()].into();
	bind(&mut fixture, "page_image", view.clone());
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.local_authority = true;
	state.scoped = true;
	state.permit_tools = true;
	*state.tool_call_approvals.lock().unwrap() =
		[ToolCallApproval::Approved { approval_id: None }].into();
	state.settled_research = Some(aidash_domain::research::expiry::tombstone(Operation::View));
	respond(
		&mut fixture.run,
		vec![call(
			"image-0",
			"page_image",
			json!({"document_id":EXPIRED_DOCUMENT, "page":1}),
		)],
	);
	if replay {
		let key = format!("{}:0:0", fixture.run.id);
		fixture
			.backend
			.0
			.invocations
			.lock()
			.unwrap()
			.insert(key, ("COMPLETED".into(), Some(output)));
	}
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.state.tool().unwrap().selected_media.is_empty());
	assert!(
		matches!(fixture.run.context.events().last(), Some(ContextEvent::Tool{result,..}) if result["error"]["code"] == "result_expired")
	);
	assert_eq!(view.invocations(), usize::from(!replay));
}

#[rstest]
#[tokio::test]
async fn provider_tools_reported_sources_cannot_publish_source_citations(mut fixture: Fixture) {
	let declaration =
		aidash_domain::tool::provider_tools::declaration("openrouter:web_search").unwrap();
	let descriptor = aidash_domain::tool::providers::ToolDescriptor {
		registry_deployment: NODE.into(),
		provider: declaration.id.into(),
		operation: declaration.alias.into(),
		default_alias: declaration.alias.into(),
		tier: aidash_domain::tool::providers::ToolTier::Provider,
		narrow: Default::default(),
		transport: None,
		lifecycle: None,
	};
	let mut tool = Scripted::new(
		ToolSpec {
			name: "provider_search".into(),
			description: "Provider search".into(),
			parameters: json!({"type":"object"}),
		},
		"workspace_read",
	);
	Arc::get_mut(&mut tool).unwrap().contract = descriptor
		.declared_contract(QualifiedRef {
			registry_deployment: NODE.into(),
			id: "provider.openrouter.web_search".into(),
			version: "1.0.0".into(),
		})
		.unwrap();
	bind(&mut fixture, "provider_search", tool);
	answer(
		&mut fixture.run,
		format!("Reported source [[cite:{UNKNOWN}]]."),
	);
	fixture
		.run
		.state
		.tool_mut()
		.unwrap()
		.response
		.provider_activity = vec![
		json!({"type":"url_citation","url_citation":{"url":"https://example.com","id":UNKNOWN.to_string()}}),
	];
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(last_write(&fixture).0, "run.citation_correction_required");
	assert!(fixture.backend.0.artifacts.lock().unwrap().is_empty());
	assert_eq!(checks(&fixture), 1);
	assert!(
		last_write(&fixture)
			.1
			.context
			.events()
			.any(|event| matches!(
				event, ContextEvent::Assistant { provider_activity, .. } if !provider_activity.is_empty()
			)),
		"a rejected citation must not discard accepted Provider activity"
	);
	assert!(
		last_write(&fixture)
			.1
			.context
			.events()
			.all(|event| !matches!(
				event, ContextEvent::Assistant { text, .. } if text.contains("Reported source")
			)),
		"the answer the citation gate rejected must not be recorded"
	);
}
