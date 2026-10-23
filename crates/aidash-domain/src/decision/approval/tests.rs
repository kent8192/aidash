use super::*;

fn route(threshold: f64) -> DeciderRoute {
	DeciderRoute {
		threshold: Probability::new(threshold).unwrap(),
		..DeciderRoute::jev("typesafe/jev-1.14")
	}
}
fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Approval,
		answer_type: AnswerType::Noul,
		description: "Block risky calls".into(),
		provider_credential: "openrouter".into(),
		routes: vec![route(0.5)],
		builder: APPROVAL_BUILDER.into(),
		option_source: APPROVAL_OPTION_SOURCE.into(),
		rule: APPROVAL_RULE.into(),
		mode: Mode::Enforce,
		max_calls_per_run: 200,
		state_retention: policy::StateRetention::default(),
	}
}
/// A way to corrupt a record, named for the assertion message.
type Change = fn(&mut ApprovalEvidence);

fn answers(values: [f64; 5]) -> BTreeMap<String, Probability> {
	BlockReason::CATEGORIES
		.into_iter()
		.zip(values)
		.map(|(reason, value)| (reason.id().to_owned(), Probability::new(value).unwrap()))
		.collect()
}
fn digest_of(fill: char) -> String {
	format!("sha256:{}", fill.to_string().repeat(64))
}

#[test]
fn every_category_has_one_fixed_question_named_after_its_reason() {
	let questions = approval_questions();
	assert_eq!(
		questions.keys().map(String::as_str).collect::<Vec<_>>(),
		[
			"beyond_request",
			"boundary",
			"data_exfiltration",
			"destructive",
			"injected"
		]
	);
	for reason in BlockReason::CATEGORIES {
		let question = &questions[reason.id()];
		assert!(!question.description.trim().is_empty());
		assert_eq!(question.answer_type, AnswerType::Noul);
		assert_eq!(serde_json::to_value(reason).unwrap(), reason.id());
	}
	assert_eq!(
		serde_json::to_value(BlockReason::Unavailable).unwrap(),
		"unavailable"
	);
	assert_eq!(BlockReason::Unavailable.question(), None);
	assert_eq!(questions, approval_questions());
}

#[test]
fn a_blocked_call_is_refused_with_a_message_naming_its_reason() {
	for reason in BlockReason::CATEGORIES
		.into_iter()
		.chain([BlockReason::Unavailable])
	{
		assert_eq!(
			reason.refusal_message(),
			format!("tool call was blocked by Auto: {}", reason.id())
		);
	}
}

#[test]
fn a_category_at_or_above_the_threshold_blocks_and_the_most_probable_names_the_reason() {
	let half = Probability::half();
	assert_eq!(
		verdict(&answers([0.1, 0.2, 0.3, 0.4, 0.49]), half).unwrap(),
		Verdict::Allowed
	);
	// The threshold is inclusive, down to the neighbouring binary64 value.
	assert_eq!(
		verdict(&answers([0.0, 0.0, 0.0, 0.5, 0.0]), half).unwrap(),
		Verdict::Blocked(BlockReason::Injected)
	);
	let below = f64::from_bits(half.value().to_bits() - 1);
	assert_eq!(
		verdict(&answers([below, 0.0, 0.0, 0.0, 0.0]), half).unwrap(),
		Verdict::Allowed
	);
	// Several categories fire: the most probable one wins.
	assert_eq!(
		verdict(&answers([0.6, 0.9, 0.7, 0.5, 0.8]), half).unwrap(),
		Verdict::Blocked(BlockReason::Destructive)
	);
	// A tie goes to the first category in the rule's order, whatever the map order.
	assert_eq!(
		verdict(&answers([0.0, 0.7, 0.0, 0.7, 0.7]), half).unwrap(),
		Verdict::Blocked(BlockReason::Destructive)
	);
	assert_eq!(
		verdict(&answers([0.7, 0.7, 0.7, 0.7, 0.7]), half).unwrap(),
		Verdict::Blocked(BlockReason::DataExfiltration)
	);
	// A lower threshold blocks what a higher one allows.
	let strict = Probability::new(0.2).unwrap();
	let probabilities = answers([0.0, 0.0, 0.25, 0.0, 0.0]);
	assert_eq!(verdict(&probabilities, half).unwrap(), Verdict::Allowed);
	assert_eq!(
		verdict(&probabilities, strict).unwrap(),
		Verdict::Blocked(BlockReason::BeyondRequest)
	);
}

#[test]
fn a_verdict_needs_an_answer_to_every_question_and_no_other() {
	let mut missing = answers([0.0; 5]);
	missing.remove("boundary");
	let mut extra = answers([0.0; 5]);
	extra.insert("unavailable".into(), Probability::half());
	for incomplete in [missing, extra, BTreeMap::new()] {
		assert!(verdict(&incomplete, Probability::half()).is_err());
	}
	assert_eq!(Verdict::Allowed.reason(), None);
	assert_eq!(
		Verdict::Blocked(BlockReason::Boundary).reason(),
		Some(BlockReason::Boundary)
	);
}

#[test]
fn an_approval_decider_names_the_approval_contract_and_keeps_no_state() {
	config().validate().unwrap();
	let json = serde_json::to_value(config()).unwrap();
	assert_eq!(json["hook"], "approval");
	assert_eq!(
		serde_json::from_value::<DeciderConfig>(json).unwrap(),
		config()
	);
	let mismatches: [fn(&mut DeciderConfig); 5] = [
		|c| c.builder = BUILDER.into(),
		|c| c.option_source = OPTION_SOURCE.into(),
		|c| c.rule = RULE.into(),
		|c| c.state_retention.enabled = true,
		|c| c.hook = Hook::Compaction,
	];
	for change in mismatches {
		let mut invalid = config();
		change(&mut invalid);
		assert!(invalid.validate().is_err(), "{invalid:?}");
	}
	// The compaction contract stays as it was, and neither hook takes the other's.
	let mut compaction = config();
	compaction.hook = Hook::Compaction;
	compaction.builder = BUILDER.into();
	compaction.option_source = OPTION_SOURCE.into();
	compaction.rule = RULE.into();
	compaction.validate().unwrap();
	compaction.hook = Hook::Approval;
	assert!(compaction.validate().is_err());
	assert_ne!(
		config().contract_digest().unwrap(),
		DeciderConfig {
			hook: Hook::Compaction,
			builder: BUILDER.into(),
			option_source: OPTION_SOURCE.into(),
			rule: RULE.into(),
			..config()
		}
		.contract_digest()
		.unwrap()
	);
}

#[test]
fn a_binding_can_only_make_an_approval_decider_stricter() {
	let config = config();
	let lenient = Restrictions::default();
	let strict = Restrictions {
		threshold: Probability::new(0.2).unwrap(),
		..Restrictions::default()
	};
	assert!(strict.at_least_as_strict(&lenient) && !lenient.at_least_as_strict(&strict));
	assert_eq!(
		strict.intersect(&lenient, &config).unwrap().threshold,
		strict.threshold
	);
	assert!(
		Restrictions {
			threshold: Probability::new(0.6).unwrap(),
			..Restrictions::default()
		}
		.validate(&config)
		.is_err()
	);
	// The Route's own threshold still applies under a laxer Binding.
	let lower = route(0.3);
	assert_eq!(lenient.with_route(&lower).threshold, lower.threshold);
	assert_eq!(strict.with_route(&lower).threshold, strict.threshold);
}

fn evidence() -> ApprovalEvidence {
	let config = DeciderConfig {
		routes: vec![route(0.5), route(0.3)],
		..config()
	};
	let restrictions = Restrictions {
		threshold: Probability::new(0.3).unwrap(),
		..Restrictions::default()
	};
	let questions = approval_questions();
	let asked: Vec<_> = questions.keys().cloned().collect();
	ApprovalEvidence {
		version: APPROVAL_EVIDENCE_VERSION,
		id: Uuid::from_u128(1),
		boundary: Boundary {
			node: "aidash://node".into(),
			run: Uuid::from_u128(2),
			step: 3,
			run_revision: 4,
			input_revision: 5,
			worker: Uuid::from_u128(6),
			input_digest: digest_of('a'),
			cycle: 0,
			retry: 0,
		},
		approval: Uuid::from_u128(7),
		decider: DeciderPin {
			identity: crate::registry::bindings::QualifiedRef::builtin(
				"aidash://node",
				"approval-decider",
			),
			definition_digest: digest_of('b'),
			configuration_digest: config.digest().unwrap(),
			provider_implementation: "node-decision-adapter:23".into(),
		},
		route: Some(1),
		failed_routes: vec![0],
		route_models: config.models(),
		route_thresholds: config.thresholds(),
		configuration_parameters_digest: config.parameters_digest().unwrap(),
		state_digest: digest_of('c'),
		answers: answers([0.0, 0.0, 0.0, 0.31, 0.0]),
		restrictions,
		binding_restrictions: Restrictions::default(),
		mode: Mode::Enforce,
		verdict: Verdict::Blocked(BlockReason::Injected),
		attempts: vec![AttemptEvidence {
			provider_report: Default::default(),
			id: Uuid::from_u128(8),
			request_digest: digest_of('d'),
			question_digest: question_digest(&questions, &asked).unwrap(),
			questions: asked,
			status: AttemptStatus::Answered,
			owner_receipts: BTreeMap::from([("local".into(), Uuid::from_u128(9))]),
		}],
	}
}

#[test]
fn evidence_replays_its_verdict_from_the_answers_and_the_effective_threshold() {
	let recorded = evidence();
	recorded.validate().unwrap();
	assert_eq!(recorded.outcome(), ApprovalOutcome::Blocked);
	let json = serde_json::to_value(&recorded).unwrap();
	assert_eq!(json["verdict"], serde_json::json!({"blocked":"injected"}));
	assert_eq!(
		serde_json::from_value::<ApprovalEvidence>(json).unwrap(),
		recorded
	);
	let mut allowed = evidence();
	allowed.answers = answers([0.0; 5]);
	allowed.verdict = Verdict::Allowed;
	allowed.validate().unwrap();
	assert_eq!(allowed.outcome(), ApprovalOutcome::Allowed);

	let changes: [(&str, Change); 14] = [
		("a verdict its answers do not give", |e| {
			e.verdict = Verdict::Allowed
		}),
		("another reason than the most probable", |e| {
			e.verdict = Verdict::Blocked(BlockReason::Destructive)
		}),
		("a threshold looser than the Route's", |e| {
			e.restrictions.threshold = Probability::new(0.4).unwrap()
		}),
		("a threshold looser than the Binding's", |e| {
			e.binding_restrictions.threshold = Probability::new(0.2).unwrap()
		}),
		("an unsupported version", |e| e.version += 1),
		("a nil approval", |e| e.approval = Uuid::nil()),
		("a Route that does not exist", |e| e.route = Some(2)),
		("a failed Route not before the answering one", |e| {
			e.failed_routes = vec![1]
		}),
		("failed Routes out of order", |e| {
			e.failed_routes = vec![0, 0]
		}),
		("a missing answer", |e| {
			e.answers.remove("boundary");
		}),
		("another mode than the pin", |e| e.mode = Mode::Shadow),
		("other Routes than the pin", |e| {
			e.route_models[0] = "typesafe/jev-1.13".into()
		}),
		("a Decider from another Node", |e| {
			e.decider.identity.registry_deployment = "aidash://other".into()
		}),
		("answers no attempt covers", |e| e.attempts.clear()),
	];
	for (what, change) in changes {
		let mut corrupt = evidence();
		change(&mut corrupt);
		assert!(corrupt.validate().is_err(), "{what}");
	}
}

#[test]
fn an_unavailable_route_keeps_the_answers_its_attempts_recorded() {
	let mut none = evidence();
	none.route = None;
	none.failed_routes = vec![0];
	none.answers = answers([0.0, 0.0, 0.25, 0.0, 0.0]);
	none.attempts.clear();
	none.verdict = Verdict::Blocked(BlockReason::Unavailable);
	none.validate().unwrap();
	assert_eq!(none.outcome(), ApprovalOutcome::Blocked);
}

#[test]
fn without_a_verdict_from_any_route_the_call_is_blocked_as_unavailable() {
	let mut none = evidence();
	none.route = None;
	none.failed_routes = vec![0, 1];
	none.answers.clear();
	none.attempts.clear();
	none.verdict = Verdict::Blocked(BlockReason::Unavailable);
	none.validate().unwrap();
	assert_eq!(none.outcome(), ApprovalOutcome::Blocked);
	let changes: [(&str, Change); 3] = [
		("another verdict", |e| e.verdict = Verdict::Allowed),
		("a failed Route that does not exist", |e| {
			e.failed_routes = vec![0, 2]
		}),
		("an answer no question names", |e| {
			e.answers.insert("unavailable".into(), Probability::half());
		}),
	];
	for (what, change) in changes {
		let mut corrupt = none.clone();
		change(&mut corrupt);
		assert!(corrupt.validate().is_err(), "{what}");
	}
}

#[test]
fn shadow_records_the_verdict_without_acting_on_it() {
	let config = DeciderConfig {
		mode: Mode::Shadow,
		routes: vec![route(0.5), route(0.3)],
		..config()
	};
	let mut shadow = evidence();
	shadow.mode = Mode::Shadow;
	shadow.decider.configuration_digest = config.digest().unwrap();
	shadow.validate().unwrap();
	assert_eq!(shadow.outcome(), ApprovalOutcome::Shadow);
	assert_eq!(shadow.verdict, Verdict::Blocked(BlockReason::Injected));
}
