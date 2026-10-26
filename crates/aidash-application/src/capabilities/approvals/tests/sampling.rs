use super::*;
use aidash_domain::agent_plugin::interaction::{SamplingRequest, SamplingTarget};

fn target() -> SamplingTarget {
	SamplingTarget {
		agent: scope(Value::Null).run.agent_id,
		plugin: "acme".to_owned().try_into().unwrap(),
		server: "docs".to_owned().try_into().unwrap(),
		digest: format!("sha256:{}", "a".repeat(64)),
		origin: "https://example.com".into(),
	}
}
fn request(scope: &Fake, key: &str) -> SamplingApprovalRequest {
	SamplingApprovalRequest { key: key.into(), target: SamplingTarget { agent: scope.run.agent_id.clone(), ..target() }, request: SamplingRequest::parse(&json!({"messages":[{"role":"user","content":{"type":"text","text":"server message"}}],"systemPrompt":"server system","maxTokens":32}), 16).unwrap(), expires_at: Utc::now() + Duration::minutes(5), area: None }
}
fn sampling_scope() -> Fake {
	let mut scope = tool_scope(Value::Null);
	scope.bundle.policies.push(serde_json::from_value(json!({"id":"sampling-approver","effect":"allow","subjects":{"ids":["bob"]},"actions":["capability.approve"],"resources":{"kinds":["sampling"]}})).unwrap());
	scope
}
async fn ask_sample(scope: &mut Fake, request: SamplingApprovalRequest) -> ToolCallApproval {
	let run = scope.run.clone();
	super::super::sampling::request(scope, &run, request)
		.await
		.unwrap()
}

#[tokio::test]
async fn a_sampling_grant_covers_only_its_server_copy_and_survives_new_input() {
	let mut scope = sampling_scope();
	let first = request(&scope, "first");
	let ToolCallApproval::Pending { approval_id, .. } = ask_sample(&mut scope, first.clone()).await
	else {
		panic!("Bob must approve");
	};
	let record = scope.records[&approval_id].clone();
	assert_eq!(record.kind, SAMPLING);
	assert_eq!(
		record.data["sampling"]["messages"][0]["text"],
		"server message"
	);
	assert_eq!(record.data["sampling"]["max_tokens"], 16);
	assert_eq!(record.data["include_context"], "none");
	scope.principal = "bob".into();
	decide(
		&mut scope,
		approval_id,
		ApprovalDecision {
			idempotency_key: Uuid::new_v4(),
			expected_revision: record.revision,
			choice: ApprovalChoice::AllowRun,
			targets: Some(vec![first.target.allow_target()]),
			expires_at: Some(Utc::now() + Duration::minutes(10)),
		},
	)
	.await
	.unwrap();
	scope.principal = "alice".into();
	scope.run.observed_input_seq += 1;
	let next = request(&scope, "after-input");
	assert!(matches!(
		ask_sample(&mut scope, next).await,
		ToolCallApproval::Approved { .. }
	));
	for variant in ["server", "copy", "origin"] {
		let mut changed = request(&scope, variant);
		match variant {
			"server" => changed.target.server = "other".to_owned().try_into().unwrap(),
			"copy" => changed.target.digest = format!("sha256:{}", "b".repeat(64)),
			_ => changed.target.origin = "https://other.example.com".into(),
		}
		assert!(
			matches!(
				ask_sample(&mut scope, changed).await,
				ToolCallApproval::Pending { .. }
			),
			"{variant}"
		);
	}
}

#[tokio::test]
async fn an_individual_allow_pins_the_copy_while_a_deny_follows_the_server_name() {
	let mut scope = sampling_scope();
	allow_alice(
		&mut scope,
		&["capability.approve", APPROVAL_ADVANCE],
		&["sampling"],
	);
	let first = request(&scope, "same-copy");
	scope.settings = Some(Settings {
		mode: ApprovalMode::Manual,
		rules: vec![ApprovalRule {
			id: Uuid::new_v4(),
			kind: ApprovalKind::Sampling,
			target: RuleTarget::Exact(first.target.allow_target()),
			effect: RuleEffect::Allow,
		}],
		bypass_accepted: false,
		auto_available: true,
	});
	assert!(matches!(
		ask_sample(&mut scope, first.clone()).await,
		ToolCallApproval::Approved { .. }
	));
	scope.run.agent_version = "2.0.0".into();
	let next_version = request(&scope, "new-agent-version");
	assert!(matches!(
		ask_sample(&mut scope, next_version).await,
		ToolCallApproval::Approved { .. }
	));
	let mut changed = request(&scope, "new-copy");
	changed.target.digest = format!("sha256:{}", "b".repeat(64));
	assert!(matches!(
		ask_sample(&mut scope, changed.clone()).await,
		ToolCallApproval::Pending { .. }
	));
	scope.settings.as_mut().unwrap().rules = vec![ApprovalRule {
		id: Uuid::new_v4(),
		kind: ApprovalKind::Sampling,
		target: RuleTarget::Exact(first.target.deny_target()),
		effect: RuleEffect::Deny,
	}];
	changed.key = "deny-updated-copy".into();
	changed.target.origin = "https://changed.example.com".into();
	assert_eq!(
		ask_sample(&mut scope, changed).await,
		ToolCallApproval::Refused(Refusal::Rule)
	);
}

#[rstest::rstest]
#[case(ApprovalMode::Auto, false)]
#[case(ApprovalMode::DontAsk, true)]
#[tokio::test]
async fn sampling_auto_asks_a_person_and_dont_ask_refuses(
	#[case] mode: ApprovalMode,
	#[case] refused: bool,
) {
	let mut scope = sampling_scope();
	allow_alice(
		&mut scope,
		&["capability.approve", APPROVAL_ADVANCE],
		&["sampling"],
	);
	allow_alice(&mut scope, &[MODE_AUTO], &[MODE_RESOURCE]);
	scope.settings = Some(Settings {
		mode,
		rules: vec![],
		bypass_accepted: false,
		auto_available: true,
	});
	let input = request(&scope, "mode");
	let result = ask_sample(&mut scope, input).await;
	assert_eq!(
		matches!(result, ToolCallApproval::Refused(Refusal::DontAsk)),
		refused
	);
	if !refused {
		assert!(matches!(result, ToolCallApproval::Pending { .. }));
	}
	assert!(
		scope
			.records
			.values()
			.all(|record| record.data["judge"] != true)
	);
}
