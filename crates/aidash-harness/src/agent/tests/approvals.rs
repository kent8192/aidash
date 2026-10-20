//! A call of a Tenant's HTTP or MCP tool is journaled only once a Tool call
//! approval admits it, and a Run executing for another Node still asks that Node.
use super::decider::ApprovalDecider;
use super::*;
use aidash_application::capabilities::approvals::{Refusal, Settlement as Settled};
use aidash_domain::approval_rules::{ApprovalKind, ApprovalMode};
use aidash_domain::decision::{ApprovalOutcome, BlockReason, Mode, Verdict};

fn tenant_tool(fixture: &mut Fixture) -> Arc<Scripted> {
	let tool = Scripted::tenant("crm");
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert("crm".into(), tool.clone());
	state.scoped = true;
	state.permit_tools = true;
	tool
}

fn approvals(fixture: &Fixture, answers: impl IntoIterator<Item = ToolCallApproval>) {
	*fixture.backend.0.tool_call_approvals.lock().unwrap() = answers.into_iter().collect();
}

fn base_key(run: &Run) -> String {
	format!("{}:0:0", run.id)
}

fn waiting(run: &Run) -> &WaitingState {
	let RunState::Waiting(waiting) = &run.state else {
		panic!("the Run waits, not {:?}", run.phase());
	};
	waiting
}

#[rstest]
#[tokio::test]
async fn an_approved_call_runs_once_under_its_ordinary_key(mut fixture: Fixture) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	*tool.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);
	let arguments = json!({"customer":"c-1"});
	respond(
		&mut fixture.run,
		vec![call("call-0", "crm", arguments.clone())],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.keys(), [base_key(&fixture.run)]);
	let requests = fixture.backend.0.tool_call_requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("one approval request");
	};
	assert_eq!(request.key, base_key(&fixture.run));
	assert_eq!(
		request.tool,
		EntityRef {
			id: "crm".into(),
			version: "1.0.0".into()
		}
	);
	assert_eq!(request.operation, "invoke");
	assert_eq!(request.call.arguments, arguments);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if *result == json!({"status":"ok"})
	));
}

#[rstest]
#[tokio::test]
async fn a_call_waits_unjournaled_for_its_approval_and_then_runs_without_a_new_response(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	*tool.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	let approval = Uuid::from_u128(0xa1);
	approvals(
		&fixture,
		[
			ToolCallApproval::Pending {
				approval_id: approval,
				expires_at: "2026-10-30T00:00:00Z".parse().unwrap(),
			},
			ToolCallApproval::Approved {
				approval_id: Some(approval),
			},
		],
	);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_tool_approval");
	let WaitingState::Approval {
		approval: reference,
		resume,
	} = waiting(&saved)
	else {
		panic!("the Run waits for an approval");
	};
	assert!(
		matches!(reference, ApprovalRef::Capability { approval_id } if *approval_id == approval)
	);
	assert_eq!((resume.cursor, resume.approval), (0, None));
	assert_eq!(tool.invocations(), 0);
	assert!(
		!fixture
			.backend
			.0
			.invocations
			.lock()
			.unwrap()
			.contains_key(&base_key(&saved)),
		"nothing is journaled before the approval"
	);

	// The approval is decided and the Run wakes into the same call.
	advance_sources(&mut fixture).await.unwrap();
	assert!(matches!(&fixture.run.state, RunState::ToolCall(state) if state.cursor == 0));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.keys(), [base_key(&fixture.run)]);
	assert_eq!(last_write(&fixture).0, "run.tool_recorded");
	assert!(
		!fixture.backend.calls().contains(&"inference.start"),
		"the approved call needs no further model response"
	);
	let requests = fixture.backend.0.tool_call_requests.lock().unwrap();
	assert_eq!(requests.len(), 2);
	assert_eq!(requests[0].key, requests[1].key);
	assert_eq!(requests[0].call, requests[1].call);
}

#[rstest]
#[case::denied(Refusal::Denied, "tool call approval was denied")]
#[case::expired(Refusal::Expired, "tool call approval expired")]
#[case::forbidden(Refusal::Forbidden, "tool call was refused by the Tenant's policy")]
#[tokio::test]
async fn a_refused_call_is_reported_to_the_model_and_never_runs(
	mut fixture: Fixture,
	#[case] refusal: Refusal,
	#[case] message: &str,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	approvals(&fixture, [ToolCallApproval::Refused(refusal)]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 0);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if *result == json!({"error":message})
	));
	assert!(matches!(&saved.state, RunState::ToolCall(state) if state.cursor == 1));
}

#[rstest]
#[case::answered_in_time(1, true, true)]
#[case::answered_after_the_deadline(16, true, false)]
#[case::refused(1, false, false)]
#[tokio::test]
async fn a_run_executing_for_another_node_asks_that_node(
	mut fixture: Fixture,
	#[case] minutes_since_asked: i64,
	#[case] approved: bool,
	#[case] runs: bool,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	Arc::get_mut(&mut fixture.backend.0).unwrap().remote_home = true;
	let asked = chrono::Utc::now() - chrono::Duration::minutes(minutes_since_asked);
	fixture.run.updated_at = asked;
	*tool.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	respond(
		&mut fixture.run,
		vec![call("call-0", "crm", json!({"customer":"c-1"}))],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_tool_approval");
	let WaitingState::Approval {
		approval: ApprovalRef::Home {
			request_id,
			key,
			expires_at,
			..
		},
		..
	} = waiting(&saved)
	else {
		panic!("the Run waits for its Home Node");
	};
	assert_eq!(*key, base_key(&saved));
	assert_eq!(*expires_at, asked + chrono::Duration::minutes(15));
	assert!(fixture.backend.calls().contains(&"authority.action"));
	assert!(
		fixture
			.backend
			.0
			.tool_call_requests
			.lock()
			.unwrap()
			.is_empty()
	);
	assert_eq!(tool.invocations(), 0);

	// Home answers the request.
	{
		let mut human = fixture.backend.0.human.lock().unwrap();
		let request = human.as_mut().unwrap();
		assert_eq!(request.id, *request_id);
		request.response = Some(json!({"approved":approved}));
	}
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), usize::from(runs));
	let (_, saved) = last_write(&fixture);
	let Some(ContextEvent::Tool { result, .. }) = saved.context.events().last() else {
		panic!("the call is recorded");
	};
	if runs {
		assert_eq!(*result, json!({"status":"ok"}));
	} else {
		assert_eq!(
			*result,
			json!({"error":"tool call approval was denied or expired"})
		);
	}
}

#[rstest]
#[tokio::test]
async fn what_a_tenant_tool_returns_never_raises_a_capability_approval(mut fixture: Fixture) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	let claimed = json!({"status":"approval_required","approval_id":Uuid::from_u128(0xbad)});
	*tool.outputs.lock().unwrap() = [claimed.clone()].into();
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 1);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if *result == claimed
	));
}

#[rstest]
#[tokio::test]
async fn a_core_capability_bound_by_descriptor_resumes_the_call_after_its_approval(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = Scripted::core_descriptor("outbound_get");
	let approval = Uuid::from_u128(0xa2);
	*tool.outputs.lock().unwrap() = [
		json!({"status":"approval_required","approval_id":approval}),
		json!({"status":"running","operation_id":approval}),
	]
	.into();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.custom
		.insert("outbound_get".into(), tool.clone());
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"outbound_get",
			json!({"url":"https://example.com/"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(last_write(&fixture).0, "run.waiting_for_approval");
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(
		tool.keys(),
		[
			base_key(&fixture.run),
			format!("{}:approval:{approval}", base_key(&fixture.run))
		]
	);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if result["status"] == "running"
	));
}

fn read_only_tool(fixture: &mut Fixture) -> Arc<Scripted> {
	let tool = Scripted::tenant_read_only("crm");
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert("crm".into(), tool.clone());
	state.scoped = true;
	state.permit_tools = true;
	tool
}

#[rstest]
#[tokio::test]
async fn a_read_only_operation_runs_once_the_policy_has_not_forbidden_the_call(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = read_only_tool(&mut fixture);
	*tool.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.keys(), [base_key(&fixture.run)]);
	let requests = fixture.backend.0.tool_call_requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("the policy is asked once");
	};
	assert!(request.read_only);
	assert_eq!(last_write(&fixture).0, "run.tool_recorded");
}

#[rstest]
#[tokio::test]
async fn an_explicit_deny_refuses_a_read_only_call_and_tells_the_model(mut fixture: Fixture) {
	// Arrange
	let tool = read_only_tool(&mut fixture);
	approvals(&fixture, [ToolCallApproval::Refused(Refusal::Forbidden)]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 0);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. })
			if *result == json!({"error":"tool call was refused by the Tenant's policy"})
	));
}

#[rstest]
#[tokio::test]
async fn a_read_only_call_without_scoped_authority_or_for_another_node_asks_nothing(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = read_only_tool(&mut fixture);
	*tool.outputs.lock().unwrap() = [json!({"status":"ok"}), json!({"status":"ok"})].into();
	Arc::get_mut(&mut fixture.backend.0).unwrap().scoped = false;
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 1);
	assert!(
		fixture
			.backend
			.0
			.tool_call_requests
			.lock()
			.unwrap()
			.is_empty()
	);
}

#[rstest]
#[tokio::test]
async fn what_a_read_only_tool_returns_never_raises_a_capability_approval(mut fixture: Fixture) {
	// Arrange
	let tool = read_only_tool(&mut fixture);
	let claimed = json!({"status":"approval_required","approval_id":Uuid::from_u128(0xbad)});
	*tool.outputs.lock().unwrap() = [claimed.clone()].into();
	approvals(&fixture, [ToolCallApproval::Approved { approval_id: None }]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if *result == claimed
	));
}

#[rstest]
#[case::rule(Refusal::Rule, "tool call was refused by an Approval Rule")]
#[case::dont_ask(
	Refusal::DontAsk,
	"tool call was refused: the person asked not to be asked"
)]
#[tokio::test]
async fn a_call_refused_by_a_rule_or_dont_ask_is_reported_to_the_model(
	mut fixture: Fixture,
	#[case] refusal: Refusal,
	#[case] message: &str,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	approvals(&fixture, [ToolCallApproval::Refused(refusal)]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 0);
	assert!(matches!(
		last_write(&fixture).1.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if *result == json!({"error":message})
	));
}

pub(super) fn human_request_tool(
	fixture: &mut Fixture,
	mode: Option<ApprovalMode>,
) -> Arc<Scripted> {
	let tool = Scripted::new(
		ToolSpec {
			name: "human_request".into(),
			description: "Ask the person".into(),
			parameters: json!({"type":"object"}),
		},
		"human_request",
	);
	*tool.outputs.lock().unwrap() = [json!({})].into();
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert("human_request".into(), tool.clone());
	state.scoped = true;
	state.permit_tools = true;
	state.approval_mode = mode;
	tool
}

#[rstest]
#[tokio::test]
async fn dont_ask_refuses_the_models_question_and_tells_it_to_continue(mut fixture: Fixture) {
	// Arrange
	let tool = human_request_tool(&mut fixture, Some(ApprovalMode::DontAsk));
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"human_request",
			json!({"kind":"QUESTION","prompt":"Which one?"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 0);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::Tool { result, .. }) if *result == json!({
			"error":"the person asked not to be asked; continue on your own judgment"
		})
	));
	assert!(!fixture.backend.calls().contains(&"home.human_request"));
}

#[rstest]
#[case::manual(Some(ApprovalMode::Manual))]
#[case::auto(Some(ApprovalMode::Auto))]
#[case::bypass(Some(ApprovalMode::Bypass))]
#[case::no_person_covers_the_run(None)]
#[tokio::test]
async fn any_other_mode_lets_the_model_ask(
	mut fixture: Fixture,
	#[case] mode: Option<ApprovalMode>,
) {
	// Arrange
	let tool = human_request_tool(&mut fixture, mode);
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"human_request",
			json!({"kind":"QUESTION","prompt":"Which one?"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 1);
}

#[rstest]
#[tokio::test]
async fn another_nodes_person_is_not_asked_for_a_mode_here(mut fixture: Fixture) {
	// Arrange
	let tool = human_request_tool(&mut fixture, Some(ApprovalMode::DontAsk));
	Arc::get_mut(&mut fixture.backend.0).unwrap().remote_home = true;
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"human_request",
			json!({"kind":"QUESTION","prompt":"Which one?"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 1);
	assert!(!fixture.backend.calls().contains(&"authority.approval_mode"));
}

fn judged(kind: ApprovalKind, target: &str) -> JudgeRequest {
	JudgeRequest {
		kind,
		target: target.into(),
		people: vec![RunInput {
			seq: 1,
			sender: "alice".into(),
			content: "Look up customer c-1 and nothing else".into(),
			idempotency_key: "key-1".into(),
			message_id: None,
			reference_only: false,
		}],
	}
}

/// The Run's approval Decider answers `decider`, and `judgements` of its
/// approvals are the Decider's to judge.
fn auto(fixture: &mut Fixture, decider: ApprovalDecider, judgements: usize, kind: ApprovalKind) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.approval_decider = Some(decider);
	state.approval_judgements = Mutex::new(
		std::iter::repeat_n(Some(judged(kind, "tool:crm@1.0.0#invoke")), judgements).collect(),
	);
}

fn pending(approval: Uuid) -> ToolCallApproval {
	ToolCallApproval::Pending {
		approval_id: approval,
		expires_at: "2026-10-30T00:00:00Z".parse().unwrap(),
	}
}

fn settlements(fixture: &Fixture) -> Vec<Settled> {
	fixture.backend.0.settlements.lock().unwrap().clone()
}

fn evidence(fixture: &Fixture) -> Vec<aidash_domain::decision::ApprovalEvidence> {
	fixture.backend.0.approval_evidence.lock().unwrap().clone()
}

fn tool_result(run: &Run) -> Value {
	let Some(ContextEvent::Tool { result, .. }) = run.context.events().last() else {
		panic!("the call is recorded");
	};
	result.clone()
}

#[rstest]
#[tokio::test]
async fn an_allowed_call_runs_at_once_without_waiting_or_another_model_response(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	*tool.outputs.lock().unwrap() = [json!({"status":"ok"})].into();
	auto(
		&mut fixture,
		ApprovalDecider::allowing(Mode::Enforce),
		1,
		ApprovalKind::ToolCall,
	);
	let approval = Uuid::from_u128(0xa1);
	approvals(
		&fixture,
		[
			pending(approval),
			ToolCallApproval::Approved {
				approval_id: Some(approval),
			},
		],
	);
	respond(
		&mut fixture.run,
		vec![call("call-0", "crm", json!({"customer":"c-1"}))],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.keys(), [base_key(&fixture.run)]);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert_eq!(tool_result(&saved), json!({"status":"ok"}));
	assert!(matches!(&saved.state, RunState::ToolCall(state) if state.cursor == 1));
	assert!(
		!fixture.backend.calls().contains(&"inference.start"),
		"no further model response is needed"
	);
	let recorded = evidence(&fixture);
	let [decision] = recorded.as_slice() else {
		panic!("one decision");
	};
	assert_eq!(decision.verdict, Verdict::Allowed);
	assert_eq!(decision.approval, approval);
	assert_eq!(
		settlements(&fixture),
		[Settled::Allowed {
			evidence: decision.id
		}]
	);
	let requests = fixture.backend.0.tool_call_requests.lock().unwrap();
	assert_eq!(requests.len(), 2, "the call asks again once it is settled");
	assert_eq!(requests[0].key, requests[1].key);
}

#[rstest]
#[tokio::test]
async fn the_decider_is_told_what_people_wrote_and_the_call_it_judges(mut fixture: Fixture) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	*tool.outputs.lock().unwrap() = [json!({})].into();
	auto(
		&mut fixture,
		ApprovalDecider::allowing(Mode::Enforce),
		1,
		ApprovalKind::ToolCall,
	);
	let approval = Uuid::from_u128(0xa1);
	approvals(
		&fixture,
		[
			pending(approval),
			ToolCallApproval::Approved {
				approval_id: Some(approval),
			},
		],
	);
	respond(
		&mut fixture.run,
		vec![call("call-0", "crm", json!({"customer":"c-9"}))],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let states = fixture.backend.0.judged_states.lock().unwrap().clone();
	let [state] = states.as_slice() else {
		panic!("one state");
	};
	let text = state.to_string();
	assert!(text.contains("Look up customer c-1 and nothing else"));
	assert!(text.contains("Do the task"));
	assert!(text.contains("c-9"));
	assert!(text.contains("Tenant HTTP fixture"));
	assert!(text.contains("tool:crm@1.0.0#invoke"));
}

#[rstest]
#[case::category(
	ApprovalDecider::flagging(BlockReason::Destructive),
	BlockReason::Destructive
)]
#[case::no_verdict(ApprovalDecider::unavailable(), BlockReason::Unavailable)]
#[tokio::test]
async fn a_blocked_call_is_refused_with_a_typed_message_and_the_run_continues(
	mut fixture: Fixture,
	#[case] decider: ApprovalDecider,
	#[case] reason: BlockReason,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	auto(&mut fixture, decider, 1, ApprovalKind::ToolCall);
	approvals(
		&fixture,
		[
			pending(Uuid::from_u128(0xa1)),
			ToolCallApproval::Refused(Refusal::Auto(reason)),
		],
	);
	respond(
		&mut fixture.run,
		vec![call("call-0", "crm", json!({"customer":"c-1"}))],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.invocations(), 0);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert_eq!(
		tool_result(&saved),
		json!({"error":format!("tool call was blocked by Auto: {}", reason.id())})
	);
	assert!(
		matches!(&saved.state, RunState::ToolCall(state) if state.cursor == 1),
		"the Run goes on to its next call"
	);
	let recorded = evidence(&fixture);
	let [decision] = recorded.as_slice() else {
		panic!("one decision");
	};
	assert_eq!(decision.verdict, Verdict::Blocked(reason));
	assert_eq!(
		settlements(&fixture),
		[Settled::Blocked {
			reason,
			evidence: decision.id
		}]
	);
}

#[rstest]
#[tokio::test]
async fn shadow_records_what_the_decider_would_do_and_leaves_the_call_waiting(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	auto(
		&mut fixture,
		ApprovalDecider {
			mode: Mode::Shadow,
			..ApprovalDecider::flagging(BlockReason::Injected)
		},
		1,
		ApprovalKind::ToolCall,
	);
	let approval = Uuid::from_u128(0xa1);
	approvals(&fixture, [pending(approval)]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_tool_approval");
	assert!(matches!(
		waiting(&saved),
		WaitingState::Approval { approval: ApprovalRef::Capability { approval_id }, .. }
			if *approval_id == approval
	));
	assert_eq!(tool.invocations(), 0);
	let recorded = evidence(&fixture);
	let [decision] = recorded.as_slice() else {
		panic!("one decision");
	};
	assert_eq!(decision.outcome(), ApprovalOutcome::Shadow);
	assert_eq!(decision.verdict, Verdict::Blocked(BlockReason::Injected));
	assert_eq!(
		settlements(&fixture),
		[Settled::Shadow {
			evidence: decision.id
		}]
	);
	assert_eq!(
		fixture.backend.0.tool_call_requests.lock().unwrap().len(),
		1
	);
}

#[rstest]
#[tokio::test]
async fn a_spent_call_allowance_leaves_the_call_waiting_for_a_person(mut fixture: Fixture) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	auto(
		&mut fixture,
		ApprovalDecider::allowance_spent(),
		1,
		ApprovalKind::ToolCall,
	);
	let approval = Uuid::from_u128(0xa1);
	approvals(&fixture, [pending(approval)]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_tool_approval");
	assert!(matches!(
		waiting(&saved),
		WaitingState::Approval { approval: ApprovalRef::Capability { approval_id }, .. }
			if *approval_id == approval
	));
	assert_eq!(tool.invocations(), 0);
	assert!(settlements(&fixture).is_empty(), "nothing was refused");
	assert!(evidence(&fixture).is_empty(), "nothing was decided");
}

#[rstest]
#[tokio::test]
async fn without_an_approval_decider_auto_waits_for_a_person(mut fixture: Fixture) {
	// Arrange
	let tool = tenant_tool(&mut fixture);
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.approval_judgements = Mutex::new([Some(judged(ApprovalKind::ToolCall, "t"))].into());
	approvals(&fixture, [pending(Uuid::from_u128(0xa1))]);
	respond(&mut fixture.run, vec![call("call-0", "crm", json!({}))]);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(last_write(&fixture).0, "run.waiting_for_tool_approval");
	assert_eq!(tool.invocations(), 0);
	assert!(settlements(&fixture).is_empty());
	assert!(evidence(&fixture).is_empty());
}

#[rstest]
#[tokio::test]
async fn after_three_blocks_in_a_row_the_next_approval_waits_for_a_person(mut fixture: Fixture) {
	// Arrange: the authority hands the Decider three approvals, then none.
	let tool = tenant_tool(&mut fixture);
	auto(
		&mut fixture,
		ApprovalDecider::flagging(BlockReason::BeyondRequest),
		3,
		ApprovalKind::ToolCall,
	);
	let blocked = ToolCallApproval::Refused(Refusal::Auto(BlockReason::BeyondRequest));
	approvals(
		&fixture,
		[
			pending(Uuid::from_u128(1)),
			blocked,
			pending(Uuid::from_u128(2)),
			blocked,
			pending(Uuid::from_u128(3)),
			blocked,
			pending(Uuid::from_u128(4)),
		],
	);
	respond(
		&mut fixture.run,
		(0..4)
			.map(|n| call(&format!("call-{n}"), "crm", json!({"n":n})))
			.collect(),
	);

	// Act
	for _ in 0..4 {
		advance_sources(&mut fixture).await.unwrap();
	}

	// Assert
	assert_eq!(settlements(&fixture).len(), 3);
	assert!(
		settlements(&fixture)
			.iter()
			.all(|settled| matches!(settled, Settled::Blocked { .. }))
	);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_tool_approval");
	assert!(matches!(
		waiting(&saved),
		WaitingState::Approval { approval: ApprovalRef::Capability { approval_id }, resume }
			if *approval_id == Uuid::from_u128(4) && resume.cursor == 3
	));
	assert_eq!(tool.invocations(), 0);
}

fn outbound_tool(fixture: &mut Fixture) -> Arc<Scripted> {
	let tool = Scripted::core_descriptor("outbound_get");
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert("outbound_get".into(), tool.clone());
	state.scoped = true;
	state.permit_tools = true;
	tool
}

#[rstest]
#[tokio::test]
async fn an_allowed_capability_call_is_made_again_at_once_under_the_approvals_key(
	mut fixture: Fixture,
) {
	// Arrange
	let tool = outbound_tool(&mut fixture);
	let approval = Uuid::from_u128(0xa2);
	*tool.outputs.lock().unwrap() = [
		json!({"status":"approval_required","approval_id":approval}),
		json!({"status":"running","operation_id":approval}),
	]
	.into();
	auto(
		&mut fixture,
		ApprovalDecider::allowing(Mode::Enforce),
		1,
		ApprovalKind::Outbound,
	);
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"outbound_get",
			json!({"url":"https://example.com/"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert: the Run never waits, and the next frame makes the same call.
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.approved_by_auto");
	assert!(matches!(&saved.state, RunState::ToolCall(state)
		if state.cursor == 0 && state.approval == Some(approval)));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(
		tool.keys(),
		[
			base_key(&fixture.run),
			format!("{}:approval:{approval}", base_key(&fixture.run))
		]
	);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert_eq!(tool_result(&saved)["status"], "running");
	assert!(!fixture.backend.calls().contains(&"inference.start"));
	assert!(matches!(
		settlements(&fixture).as_slice(),
		[Settled::Allowed { .. }]
	));
}

#[rstest]
#[tokio::test]
async fn a_blocked_capability_call_is_refused_and_never_made_again(mut fixture: Fixture) {
	// Arrange
	let tool = outbound_tool(&mut fixture);
	let approval = Uuid::from_u128(0xa2);
	*tool.outputs.lock().unwrap() =
		[json!({"status":"approval_required","approval_id":approval})].into();
	auto(
		&mut fixture,
		ApprovalDecider::flagging(BlockReason::DataExfiltration),
		1,
		ApprovalKind::Outbound,
	);
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"outbound_get",
			json!({"url":"https://example.com/"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(tool.keys(), [base_key(&fixture.run)]);
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.tool_recorded");
	assert_eq!(
		tool_result(&saved),
		json!({"error":"tool call was blocked by Auto: data_exfiltration"})
	);
	assert!(matches!(&saved.state, RunState::ToolCall(state) if state.cursor == 1));
}

#[rstest]
#[tokio::test]
async fn a_capability_approval_the_decider_does_not_judge_waits_for_a_person(mut fixture: Fixture) {
	// Arrange
	let tool = outbound_tool(&mut fixture);
	let approval = Uuid::from_u128(0xa2);
	*tool.outputs.lock().unwrap() =
		[json!({"status":"approval_required","approval_id":approval})].into();
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"outbound_get",
			json!({"url":"https://example.com/"}),
		)],
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(last_write(&fixture).0, "run.waiting_for_approval");
	assert!(settlements(&fixture).is_empty());
}

#[rstest]
#[case::auto(Some(ApprovalMode::Auto), true)]
#[case::manual(Some(ApprovalMode::Manual), false)]
#[case::dont_ask(Some(ApprovalMode::DontAsk), false)]
#[case::bypass(Some(ApprovalMode::Bypass), false)]
#[case::no_person_covers_the_run(None, false)]
#[tokio::test]
async fn only_in_auto_the_model_is_told_not_to_stop_for_questions(
	mut fixture: Fixture,
	#[case] mode: Option<ApprovalMode>,
	#[case] told: bool,
) {
	// Arrange
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.approval_mode = mode;

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("one request");
	};
	assert_eq!(
		request
			.instructions
			.contains(aidash_application::capabilities::approvals::AUTO_MODE_INSTRUCTION.trim()),
		told
	);
	assert!(request.instructions.contains("Do the task"));
}

#[tokio::test]
async fn an_ordered_request_keeps_the_auto_instruction_out_of_the_cached_system_prompt() {
	// Arrange
	let mut fixture = canonical(fixture());
	ordered(&mut fixture);
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.approval_mode = Some(ApprovalMode::Auto);
	fixture.run.state = RunState::Thinking(ThinkingState::default());

	// Act
	let _ = advance_sources(&mut fixture).await;

	// Assert
	let requests = fixture.backend.0.requests.lock().unwrap();
	let [request] = requests.as_slice() else {
		panic!("one request");
	};
	let instruction = aidash_application::capabilities::approvals::AUTO_MODE_INSTRUCTION;
	assert!(!request.instructions.contains(instruction.trim()));
	let volatile: Value = serde_json::from_str(ordered_parts(request).1).unwrap();
	assert_eq!(
		volatile["turn_instructions"],
		json!(instruction.trim_start())
	);
}
