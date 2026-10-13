//! The operator handles Web research only through declared behavior: citation
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
