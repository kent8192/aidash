//! Connection waits resume the exact unexecuted call under a fresh journal key.
use super::*;
use aidash_domain::approval_rules::ApprovalMode;
use aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND;
const RESOURCE: &str = "https://example.invalid";

fn setup(fixture: &mut Fixture, mode: Option<ApprovalMode>, mcp: bool) -> Arc<Scripted> {
	let mut tool = Scripted::tenant("crm");
	Arc::get_mut(&mut tool).unwrap().mcp = mcp;
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert("crm".into(), tool.clone());
	state.scoped = true;
	state.permit_tools = true;
	state.approval_mode = mode;
	tool.connection_errors.lock().unwrap().push_back(
		aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
			resource: RESOURCE.into(),
		},
	);
	*tool.outputs.lock().unwrap() = [
		json!({"error":"connection_required","resource":RESOURCE}),
		json!({"ok":true}),
	]
	.into();
	*state.tool_call_approvals.lock().unwrap() = (0..3)
		.map(|_| ToolCallApproval::Approved { approval_id: None })
		.collect();
	respond(
		&mut fixture.run,
		vec![call("original", "crm", json!({"customer":"c-1"}))],
	);
	tool
}

#[rstest]
#[case::manual_http(ApprovalMode::Manual, false)]
#[case::auto_http(ApprovalMode::Auto, false)]
#[case::manual_mcp(ApprovalMode::Manual, true)]
#[case::auto_mcp(ApprovalMode::Auto, true)]
#[tokio::test]
async fn connection_wait_redispatches_the_unchanged_call_after_consent(
	mut fixture: Fixture,
	#[case] mode: ApprovalMode,
	#[case] mcp: bool,
) {
	let tool = setup(&mut fixture, Some(mode), mcp);
	advance_sources(&mut fixture).await.unwrap();
	let RunState::Waiting(wait) = &fixture.run.state else {
		panic!("must wait")
	};
	let WaitingState::Human {
		resume: ResumeState::ToolCall(resume),
		..
	} = wait.as_ref()
	else {
		panic!("must resume the call")
	};
	assert_eq!(resume.cursor, 0);
	assert_eq!(
		resume.response.tool_calls[0].arguments,
		json!({"customer":"c-1"})
	);
	let human = fixture.backend.0.human.lock().unwrap().clone().unwrap();
	assert_eq!(human.kind, CONNECTION_REQUEST_KIND);
	assert_eq!(
		serde_json::from_str::<Value>(&human.prompt).unwrap()["resource"],
		RESOURCE
	);
	assert_eq!(tool.invocations(), 1);
	assert!(
		fixture.backend.0.invocations.lock().unwrap().is_empty(),
		"a refusal is not cached as a tool result"
	);
	// A new worker reads only the durable typed pending state; tool JSON is never parsed.
	let pending = fixture.run.stored_pending().unwrap();
	let (state, recovery) =
		aidash_domain::run_state::decode(aidash_domain::RunPhase::Waiting, pending).unwrap();
	fixture.run.state = state;
	fixture.run.recovery = recovery;
	assert!(
		matches!(&fixture.run.state,RunState::Waiting(wait) if matches!(wait.as_ref(),WaitingState::Human {resume:ResumeState::ToolCall(state),..} if state.connection.as_ref().unwrap().cause.is_some()))
	);
	fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.response = Some(json!({"connected":true}));
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), 2);
	assert!(
		fixture.run.recovery.connection_refusals.is_empty(),
		"real successful retry clears the typed refusal"
	);
	let keys = tool.keys();
	assert_ne!(keys[0], keys[1], "the saved refusal must not be reused");
	assert!(keys[1].contains(":connection:1"));
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool { call,result }) if call.id == "original" && *result == json!({"ok":true}))
	);
}

#[rstest]
#[case::dont_ask(Some(ApprovalMode::DontAsk), true)]
#[case::bypass(Some(ApprovalMode::Bypass), true)]
#[case::no_person(None, false)]
#[tokio::test]
async fn connection_refusal_stays_a_tool_result_without_an_automatic_wait(
	mut fixture: Fixture,
	#[case] mode: Option<ApprovalMode>,
	#[case] hint: bool,
) {
	let tool = setup(&mut fixture, mode, false);
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), 1);
	assert!(fixture.backend.0.human.lock().unwrap().is_none());
	let Some(ContextEvent::Tool { result, .. }) = fixture.run.context.events().last() else {
		panic!("tool result")
	};
	assert_eq!(result["error"], "connection_required");
	assert_eq!(result.get("hint").is_some(), hint);
}

#[rstest]
#[tokio::test]
async fn connection_deadline_answers_false_and_retains_the_original_refusal(mut fixture: Fixture) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), false);
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.answer_deadline = Some(0);
	advance_sources(&mut fixture).await.unwrap();
	// Returning an expired refusal must not ask for another tool approval.
	fixture
		.backend
		.0
		.tool_call_approvals
		.lock()
		.unwrap()
		.clear();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(
		fixture
			.backend
			.0
			.human
			.lock()
			.unwrap()
			.as_ref()
			.unwrap()
			.response,
		Some(json!({"connected":false}))
	);
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), 1);
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool {result,..}) if result["error"] == "connection_required")
	);
}

#[rstest]
#[case::builtin(false)]
#[case::descriptor(true)]
#[tokio::test]
async fn explicit_connection_request_waits_under_dont_ask_and_returns_the_answer(
	mut fixture: Fixture,
	#[case] descriptor: bool,
) {
	let tool = if descriptor {
		Scripted::core_descriptor("connection_request")
	} else {
		Scripted::new(
			ToolSpec {
				name: "connection_request".into(),
				description: "Connect".into(),
				parameters: json!({"type":"object"}),
			},
			"connection_request",
		)
	};
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state
		.custom
		.insert("connection_request".into(), tool.clone());
	state.scoped = true;
	state.permit_tools = true;
	state.approval_mode = Some(ApprovalMode::DontAsk);
	state.plugin_listing = Some(aidash_domain::agent_plugin::PluginTools::default());
	fixture.run.context.plugin_tools = Some(unconnected_plugin());
	let id = Uuid::now_v7();
	*state.human.lock().unwrap() = Some(HumanRequest {
		id,
		workspace_id: fixture.run.workspace_id,
		run_id: fixture.run.id,
		kind: CONNECTION_REQUEST_KIND.into(),
		prompt:
			json!({"resource":RESOURCE,"scopes":[],"reason":"needed","tool":"connection_request"})
				.to_string(),
		response: None,
		created_at: chrono::Utc::now(),
		answered_by: None,
		answer_by: None,
	});
	*tool.outputs.lock().unwrap() = [json!({"human_request_id":id})].into();
	respond(
		&mut fixture.run,
		vec![call(
			"connect",
			"connection_request",
			json!({"resource":RESOURCE,"reason":"needed"}),
		)],
	);
	advance_sources(&mut fixture).await.unwrap();
	assert!(
		matches!(&fixture.run.state,RunState::Waiting(wait) if matches!(wait.as_ref(),WaitingState::Human {resume:ResumeState::Thinking(_),..}))
	);
	assert_eq!(tool.invocations(), 1);
	fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.response = Some(json!({"connected":true}));
	advance_sources(&mut fixture).await.unwrap();
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool {call,result}) if call.id == "connect" && *result == json!({"connected":true}))
	);
	assert!(fixture.backend.calls().contains(&"plugins.reconnect"));
	assert!(
		fixture
			.run
			.context
			.plugin_tools
			.as_ref()
			.unwrap()
			.findings
			.is_empty()
	);
}

#[rstest]
#[tokio::test]
async fn step_up_consent_uses_only_the_logical_calls_remaining_wait_budget(mut fixture: Fixture) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), false);
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.answer_deadline = Some(120);
	*tool.outputs.lock().unwrap() =
		[json!({"error":"connection_scope_insufficient","resource":RESOURCE,"scope":"read write"})]
			.into();
	*tool.connection_errors.lock().unwrap() = [
		aidash_domain::agent_plugin::ConnectionCause::ConnectionScopeInsufficient {
			resource: RESOURCE.into(),
			scope: Some("read write".into()),
		},
	]
	.into();
	let limits = aidash_domain::agent_plugin::interaction::InteractionLimits::default();
	fixture.run.state.tool_mut().unwrap().connection =
		Some(aidash_domain::run_state::ConnectionCallState {
			attempt: 1,
			waited_millis: limits.human_wait_seconds * 1000 - 1000,
			refused: None,
			..Default::default()
		});
	advance_sources(&mut fixture).await.unwrap();
	let human = fixture.backend.0.human.lock().unwrap().clone().unwrap();
	let payload: Value = serde_json::from_str(&human.prompt).unwrap();
	assert_eq!(payload["reason"], "scope");
	assert_eq!(payload["scopes"], json!(["read", "write"]));
	let pending = fixture.run.recovery.connection_wait.as_ref().unwrap();
	assert_eq!(
		pending.answer_by - human.created_at,
		chrono::Duration::seconds(1)
	);
}

#[rstest]
#[case::unavailable("connection_unavailable")]
#[case::remote("connection_remote_refused")]
#[tokio::test]
async fn other_connection_failures_are_not_replayed_or_turned_into_consent(
	mut fixture: Fixture,
	#[case] error: &str,
) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), false);
	tool.connection_errors.lock().unwrap().clear();
	let result = json!({"error":error,"resource":RESOURCE});
	*tool.outputs.lock().unwrap() = [result.clone()].into();
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.backend.0.human.lock().unwrap().is_none());
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool {result:actual,..}) if *actual == result)
	);
}

fn unconnected_plugin() -> aidash_domain::agent_plugin::PluginTools {
	aidash_domain::agent_plugin::PluginTools {
		servers: vec![],
		findings: vec![aidash_domain::agent_plugin::PluginFinding {
			plugin: "acme".to_owned().try_into().unwrap(),
			finding: aidash_domain::agent_plugin::Finding {
				component: aidash_domain::agent_plugin::Component::Server("docs".into()),
				outcome: aidash_domain::agent_plugin::Outcome::Skipped,
				reason: aidash_domain::agent_plugin::Reason::ServerUnavailable,
				detail: "connection_required".into(),
			},
			connection: Some(
				aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
					resource: RESOURCE.into(),
				},
			),
		}],
	}
}

#[rstest]
#[tokio::test]
async fn the_model_is_told_which_connection_can_make_pinned_plugin_tools_available(
	mut fixture: Fixture,
) {
	fixture.run.context.plugin_tools = Some(unconnected_plugin());
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	advance_sources(&mut fixture).await.unwrap();
	let requests = fixture.backend.0.requests.lock().unwrap();
	let request = requests.last().unwrap();
	assert!(request.instructions.contains(RESOURCE));
	assert!(request.instructions.contains("connection_request"));
	assert!(
		fixture.backend.0.human.lock().unwrap().is_none(),
		"pin findings do not wait by themselves"
	);
}

#[rstest]
#[case::http(false)]
#[case::mcp(true)]
#[tokio::test]
async fn a_successful_connection_shaped_body_clears_a_prior_refusal_without_wait_or_replay(
	mut fixture: Fixture,
	#[case] mcp: bool,
) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), mcp);
	tool.connection_errors.lock().unwrap().clear();
	let body = json!({"error":"connection_required","resource":RESOURCE});
	*tool.outputs.lock().unwrap() = [body.clone()].into();
	seed_refusal(&mut fixture.run);
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), 1);
	assert!(fixture.backend.0.human.lock().unwrap().is_none());
	assert!(fixture.run.recovery.connection_refusals.is_empty());
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool {result,..}) if result == &body)
	);
	assert!(
		fixture
			.backend
			.0
			.invocations
			.lock()
			.unwrap()
			.values()
			.any(|(status, result)| status == "COMPLETED"
				&& result.as_ref()
					== Some(&if mcp {
						serde_json::to_value(
							aidash_domain::agent_plugin::interaction::ToolRound::Complete(
								body.clone(),
							),
						)
						.unwrap()
					} else {
						body.clone()
					}))
	);
	assert!(
		fixture
			.backend
			.0
			.writes
			.lock()
			.unwrap()
			.iter()
			.any(|(event, saved)| event == "run.mcp_checkpoint"
				&& saved.recovery.connection_refusals.is_empty()),
		"real success must clear durably before its result is journaled"
	);
	assert!(aidash_application::connection_wait::failure_need(&fixture.run).is_none());
}

fn seed_refusal(run: &mut Run) {
	aidash_application::connection_wait::record_refusal(
		run,
		"crm",
		aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
			resource: RESOURCE.into(),
		},
		vec![],
	);
}

#[rstest]
#[case::http(false)]
#[case::mcp(true)]
#[tokio::test]
async fn a_rendered_invalid_error_does_not_clear_a_connection_refusal(
	mut fixture: Fixture,
	#[case] mcp: bool,
) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), mcp);
	tool.connection_errors.lock().unwrap().clear();
	tool.invalid_errors
		.lock()
		.unwrap()
		.push_back("ordinary validation error".into());
	seed_refusal(&mut fixture.run);
	let refusal = fixture.run.recovery.connection_refusals[0].clone();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), 1);
	assert_eq!(fixture.run.recovery.connection_refusals.len(), 1);
	assert_eq!(
		fixture.run.recovery.connection_refusals[0].cause,
		refusal.cause
	);
	assert!(aidash_application::connection_wait::failure_need(&fixture.run).is_some());
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool {result,..}) if result["error"] == "ordinary validation error")
	);
}

#[rstest]
#[case::http(false)]
#[case::mcp(true)]
#[tokio::test]
async fn an_earlier_cached_success_does_not_clear_a_later_connection_refusal(
	mut fixture: Fixture,
	#[case] mcp: bool,
) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), mcp);
	tool.connection_errors.lock().unwrap().clear();
	let epoch = fixture.run.state.tool().unwrap().response_epoch;
	let key = format!(
		"{}:{epoch}:0{}",
		fixture.run.id,
		if mcp { ":mcp:round:0" } else { "" }
	);
	let result = json!({"ok":"earlier success"});
	let journal = if mcp {
		serde_json::to_value(
			aidash_domain::agent_plugin::interaction::ToolRound::Complete(result.clone()),
		)
		.unwrap()
	} else {
		result.clone()
	};
	fixture
		.backend
		.0
		.invocations
		.lock()
		.unwrap()
		.insert(key, ("COMPLETED".into(), Some(journal)));
	// This refusal was recorded after that immutable success; replay is not new evidence.
	seed_refusal(&mut fixture.run);
	let refusal = fixture.run.recovery.connection_refusals[0].clone();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(tool.invocations(), 0);
	assert_eq!(fixture.run.recovery.connection_refusals.len(), 1);
	assert_eq!(
		fixture.run.recovery.connection_refusals[0].cause,
		refusal.cause
	);
	assert!(
		matches!(fixture.run.context.events().last(),Some(ContextEvent::Tool {result:actual,..}) if actual == &result)
	);
}

#[rstest]
#[tokio::test]
async fn a_connection_success_checkpoint_failure_does_not_journal_the_result(mut fixture: Fixture) {
	let tool = setup(&mut fixture, Some(ApprovalMode::Manual), false);
	tool.connection_errors.lock().unwrap().clear();
	*tool.outputs.lock().unwrap() = [json!({"ok":true})].into();
	seed_refusal(&mut fixture.run);
	*fixture.backend.0.failing_save.lock().unwrap() = Some("run.mcp_checkpoint");
	let error = advance_sources(&mut fixture).await.unwrap_err();
	assert!(matches!(error, Error::External(ref message) if message == "save failed"));
	assert_eq!(tool.invocations(), 1);
	assert!(
		fixture
			.backend
			.0
			.invocations
			.lock()
			.unwrap()
			.values()
			.all(|(status, _)| status == "STARTED"),
		"the successful effect remains reconcilable until its typed checkpoint commits"
	);
	assert!(
		!fixture
			.backend
			.0
			.calls
			.lock()
			.unwrap()
			.contains(&"invocation.finish")
	);
}
