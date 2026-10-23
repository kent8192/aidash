use super::*;
use aidash_domain::context::source::*;
use aidash_domain::registry::bindings::QualifiedRef;

fn candidate() -> SourceCandidate {
	let dependency = SourcePin {
		resource: "authorized-reference".into(),
		revision_digest: digest(&json!("revision-1")),
	};
	SourceCandidate {
		identity: CandidateIdentity {
			source: QualifiedRef::builtin("aidash://execution", "reference-source"),
			item: "reference".into(),
			revision: "revision-1".into(),
			range: None,
		},
		content: json!("DOCUMENT_SECRET"),
		preview: "Relevant authorized excerpt".into(),
		provenance: vec![dependency.clone()],
		dependencies: vec![dependency],
		completeness: Completeness::Complete,
		detail: None,
		mandatory: false,
	}
}
async fn select(fixture: &Fixture, mode: Mode, disclose: bool) -> Result<SourceSelection> {
	let candidates = vec![candidate()];
	let query = query_material(
		QueryPolicy::TaskInputsV1,
		"Find the relevant reference",
		&[],
		1024,
	);
	let config = DeciderConfig {
		hook: Hook::ContextSelection,
		builder: SOURCE_BUILDER.into(),
		option_source: SOURCE_OPTION.into(),
		rule: SOURCE_RULE.into(),
		..fixture.config(mode)
	};
	let definition = definition_of(&config);
	let boundary = Boundary {
		node: "aidash://execution".into(),
		run: Uuid::from_u128(42),
		step: 1,
		run_revision: 1,
		input_revision: 1,
		worker: Uuid::from_u128(17),
		input_digest: source_selection_input_digest(&candidates, &query, 4096),
		cycle: 0,
		retry: 0,
	};
	let pin = DeciderPin {
		identity: QualifiedRef {
			registry_deployment: boundary.node.clone(),
			id: definition.id.clone(),
			version: definition.version.clone(),
		},
		definition_digest: digest(&json!(definition)),
		configuration_digest: config.digest().unwrap(),
		provider_implementation: fixture.provider_implementation.clone(),
	};
	let bound = BoundDecider {
		pin,
		config,
		restrictions: Restrictions::default(),
	};
	let disclosure = Disclosure {
		sources: if disclose {
			source_selection_pins(&candidates, &query)
		} else {
			vec![]
		},
		..Default::default()
	};
	let input = Evaluation {
		boundary: &boundary,
		decider: &bound,
		definition: &definition,
		disclosure: &disclosure,
		now: Utc::now(),
	};
	fixture
		.gate()
		.select_sources(&candidates, &query, &input, 4096)
		.await
}
fn fixture(mode: Mode) -> Fixture {
	let mut fixture = Fixture::new(mode);
	let config = DeciderConfig {
		hook: Hook::ContextSelection,
		builder: SOURCE_BUILDER.into(),
		option_source: SOURCE_OPTION.into(),
		rule: SOURCE_RULE.into(),
		..fixture.config(mode)
	};
	fixture.configuration_digest = config.digest().unwrap();
	fixture
}
#[tokio::test]
async fn enforce_uses_charged_noul_answer_and_recovery_reuses_it_without_dispatch() {
	let fixture = Fixture {
		keep: Probability::half(),
		..fixture(Mode::Enforce)
	};
	let first = select(&fixture, Mode::Enforce, true).await.unwrap();
	assert_eq!(first.selected, vec![candidate().identity.key()]);
	assert!(first.evidence.failure.is_none());
	assert_eq!(first.evidence.replay().unwrap(), first.selected);
	assert_eq!(first.evidence.answers.len(), 1);
	assert_eq!(first.evidence.attempts[0].status, AttemptStatus::Answered);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
	let recovered = select(&fixture, Mode::Enforce, true).await.unwrap();
	assert_eq!(recovered.selected, first.selected);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
	assert_eq!(fixture.saved.lock().unwrap().permits.len(), 1);
}
#[tokio::test]
async fn shadow_proposal_preserves_candidates_and_forbidden_disclosure_never_dispatches() {
	let fixture = fixture(Mode::Shadow);
	let result = select(&fixture, Mode::Shadow, true).await.unwrap();
	assert_eq!(result.selected, vec![candidate().identity.key()]);
	assert!(result.evidence.proposed.is_empty());
	assert_eq!(result.evidence.replay().unwrap(), result.selected);
	let calls = fixture.calls.load(Ordering::SeqCst);
	let denied = select(&fixture, Mode::Shadow, false).await.unwrap();
	assert_eq!(
		denied.evidence.failure,
		Some(SelectionFailure::ForbiddenDisclosure)
	);
	assert_eq!(denied.selected, result.selected);
	assert_eq!(denied.evidence.replay().unwrap(), denied.selected);
	assert_eq!(fixture.calls.load(Ordering::SeqCst), calls);
}
#[tokio::test]
async fn enforce_unavailable_has_no_unscored_fallback_and_keeps_durable_uncertainty() {
	let fixture = Fixture {
		fail_transport: true,
		..fixture(Mode::Enforce)
	};
	let result = select(&fixture, Mode::Enforce, true).await.unwrap();
	assert!(result.selected.is_empty());
	assert_eq!(
		result.evidence.failure,
		Some(SelectionFailure::ProviderUnavailable)
	);
	assert_eq!(result.evidence.attempts[0].status, AttemptStatus::Uncertain);
	assert_eq!(fixture.saved.lock().unwrap().permits.len(), 1);
}
