//! A call of a Tenant's HTTP or MCP tool is journaled only once a Tool call
//! approval admits it, and a Run executing for another Node still asks that Node.
use super::*;
use aidash_application::capabilities::approvals::Refusal;

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
