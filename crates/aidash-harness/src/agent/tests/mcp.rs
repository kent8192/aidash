//! The MCP Run boundary owns sampling outcomes and cumulative human waits.
use super::*;
use aidash_application::ports::tools::PluginInputs;
use aidash_domain::agent_plugin::interaction::{
	CallCheckpoint, InputRequest, RequiredInput, ToolRound,
};
use aidash_domain::tool::{AgentPluginTool, ToolContract};

const KEY: &str = "plugin-call";

enum Reply {
	Sample,
	LiveThenModern,
	InvalidInput,
}

struct Remote(Reply);

fn form(message: &str) -> InputRequest {
	InputRequest {
		method: "elicitation/create".into(),
		params: json!({"message":message,"requestedSchema":{"type":"object","properties":{"label":{"type":"string"}},"required":["label"]}}),
	}
}

#[async_trait]
impl ExecutionTool for Remote {
	fn specification(&self) -> ToolSpec {
		ToolSpec {
			name: "plugin__remote__ask".into(),
			description: "Remote fixture".into(),
			parameters: json!({"type":"object"}),
		}
	}
	fn contract(&self) -> ToolContract {
		ToolContract::agent_plugin(
			AgentPluginTool {
				plugin: "plugin".to_owned().try_into().unwrap(),
				server: "remote".to_owned().try_into().unwrap(),
				digest: format!("sha256:{}", "0123456789abcdef".repeat(4)),
				tool: "ask".into(),
			},
			true,
		)
	}
	fn replay_safe(&self) -> bool {
		true
	}
	fn mcp_rounds(&self) -> bool {
		true
	}
	async fn invoke(&self, _run: &Run, _input: Value, _key: &str) -> Result<Value> {
		unexpected("remote tools use MCP rounds")
	}
	async fn mcp_round(
		&self,
		_run: &Run,
		_input: Value,
		_key: &str,
		checkpoint: Option<&CallCheckpoint>,
		inputs: Option<Arc<dyn PluginInputs>>,
	) -> Result<ToolRound> {
		match self.0 {
			Reply::InvalidInput => {
				let input = RequiredInput {
					request_state: Some("invalid".into()),
					requests: (0..=aidash_domain::agent_plugin::interaction::MAX_INPUTS)
						.map(|index| (index.to_string(), form("Too many forms")))
						.collect(),
				};
				// The native session adapter validates a received batch before
				// returning it to the harness and propagates the domain error.
				input.validate()?;
				Ok(ToolRound::InputRequired(input))
			}
			Reply::Sample => Ok(ToolRound::Complete(
				checkpoint.unwrap().responses["sample"].clone(),
			)),
			Reply::LiveThenModern => {
				let answer = inputs
					.unwrap()
					.request("live".into(), form("Live form"))
					.await?;
				assert_eq!(
					answer,
					json!({"action":"accept","content":{"label":"private answer"}})
				);
				Ok(ToolRound::InputRequired(RequiredInput {
					request_state: Some("continuation".into()),
					requests: [("modern".into(), form("Modern form"))].into(),
				}))
			}
		}
	}
}

fn sampling(fixture: &mut Fixture) {
	fixture.run.state = RunState::ToolCall(Box::default());
	let mut checkpoint = CallCheckpoint::new(KEY.into());
	checkpoint.accept(RequiredInput {
		request_state: Some("sample".into()),
		requests: [("sample".into(), InputRequest {
			method: "sampling/createMessage".into(),
			params: json!({"messages":[{"role":"user","content":{"type":"text","text":"Server request"}}],"maxTokens":32}),
		})].into(),
	}).unwrap();
	fixture.run.state.tool_mut().unwrap().mcp = Some(checkpoint);
	fixture
		.backend
		.0
		.tool_call_approvals
		.lock()
		.unwrap()
		.push_back(ToolCallApproval::Approved { approval_id: None });
}

async fn execute(fixture: &mut Fixture, remote: &Remote) -> Result<Option<Value>> {
	let backend = fixture.backend.clone();
	let agent = backend.agent(&backend.entry("agent")).unwrap();
	let call = call("remote", "plugin__remote__ask", json!({}));
	Executor::new(&backend)
		.mcp_call(
			&mut fixture.run,
			backend.0.token,
			super::super::mcp::Call {
				tool: remote,
				call: &call,
				key: KEY,
				agent: &agent,
				guard: &backend,
				visibility: &mut backend.clone(),
			},
		)
		.await
}

#[rstest]
#[tokio::test]
async fn a_rejected_input_batch_finishes_the_round_and_replays_its_error(mut fixture: Fixture) {
	fixture.run.state = RunState::ToolCall(Box::default());
	let expected = json!({"error":"MCP_INPUT_INVALID"});

	for _ in 0..2 {
		assert_eq!(
			execute(&mut fixture, &Remote(Reply::InvalidInput))
				.await
				.unwrap(),
			Some(expected.clone())
		);
	}

	assert_eq!(
		*fixture.backend.0.invocations.lock().unwrap(),
		BTreeMap::from([(
			format!("{KEY}:mcp:round:0"),
			(
				"COMPLETED".into(),
				Some(json!({"outcome":"complete","data":expected}))
			)
		)])
	);
	assert!(
		!fixture
			.backend
			.events()
			.iter()
			.any(|event| event == "run.waiting")
	);
}

#[rstest]
#[case::visibility(true, false, InterruptionReason::StreamError)]
#[case::resume(false, true, InterruptionReason::Revoked)]
#[case::inference(false, false, InterruptionReason::Revoked)]
#[tokio::test]
async fn sampling_resume_failures_close_the_attempt_and_invocation(
	mut fixture: Fixture,
	#[case] visibility: bool,
	#[case] resume: bool,
	#[case] reason: InterruptionReason,
) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.deny_visibility_resume = visibility;
	state.deny_resume = resume;
	state.revoke_inference_after_response = !visibility && !resume;
	sampling(&mut fixture);

	let result = execute(&mut fixture, &Remote(Reply::Sample)).await;
	if visibility {
		assert!(
			matches!(result, Err(Error::External(message)) if message == "visibility resume failed")
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}

	assert_eq!(
		fixture.backend.outcomes(),
		[vec![ProgressOutcome::Interrupted(reason)]]
	);
	let calls = fixture.backend.calls();
	assert!(position(&calls, "provider.response") < position(&calls, "reservation.settle"));
	assert!(position(&calls, "reservation.settle") < position(&calls, "inference.finish"));
	let invocations = fixture.backend.0.invocations.lock().unwrap();
	let sampling = invocations.values().next().expect("sampling journal");
	assert_eq!(
		sampling,
		&("COMPLETED".into(), Some(json!({"mcp_error":"provider"})))
	);
	assert!(matches!(
		fixture.backend.0.usage.lock().unwrap()[0].1,
		Some(UsageOutcome::Completed(_))
	));
}

#[rstest]
#[tokio::test]
async fn capped_sampling_returns_partial_text_and_max_tokens_without_a_provider_retry(
	mut fixture: Fixture,
) {
	sampling(&mut fixture);
	fixture
		.backend
		.script(vec![Step::Fail(Error::TerminalResponse(
			aidash_domain::context::recovery::Failure::OutputTruncated,
			Box::new(ModelResponse {
				text: "The partial answer".into(),
				input_tokens: 8,
				output_tokens: 32,
				usage_complete: true,
				reported: ReportedUsage {
					input_tokens: Some(8),
					output_tokens: Some(32),
					..Default::default()
				},
				..Default::default()
			}),
		))]);

	let output = execute(&mut fixture, &Remote(Reply::Sample))
		.await
		.unwrap()
		.unwrap();

	assert_eq!(
		output,
		json!({"role":"assistant","content":{"type":"text","text":"The partial answer"},"model":"fixture","stopReason":"maxTokens"})
	);
	assert_eq!(
		fixture.backend.outcomes(),
		[vec![ProgressOutcome::Accepted]]
	);
	assert_eq!(fixture.backend.0.requests.lock().unwrap().len(), 1);
	assert_eq!(
		fixture.backend.0.usage.lock().unwrap()[0].1,
		Some(UsageOutcome::Completed(ReportedUsage {
			input_tokens: Some(8),
			output_tokens: Some(32),
			..Default::default()
		}))
	);
}

#[rstest]
#[tokio::test]
async fn live_form_wait_reduces_the_next_modern_input_deadline(mut fixture: Fixture) {
	fixture.run.state = RunState::ToolCall(Box::default());
	let backend = fixture.backend.clone();
	let answer = tokio::spawn(async move {
		loop {
			if let Some(request) = backend.0.human.lock().unwrap().as_mut() {
				request.response =
					Some(json!({"action":"accept","content":{"label":"private answer"}}));
				break;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	});

	assert!(
		execute(&mut fixture, &Remote(Reply::LiveThenModern))
			.await
			.unwrap()
			.is_none()
	);
	answer.await.unwrap();

	let RunState::Waiting(wait) = &fixture.run.state else {
		panic!("the modern form waits");
	};
	let WaitingState::Human {
		resume: ResumeState::ToolCall(resume),
		..
	} = wait.as_ref()
	else {
		panic!("the MCP call resumes");
	};
	let checkpoint = resume.mcp.as_ref().unwrap();
	assert!(
		checkpoint.waited_millis >= 400,
		"the live wait was retained: {checkpoint:?}"
	);
	let active = checkpoint.active.as_ref().unwrap();
	assert_eq!(
		(active.deadline - active.started_at).num_milliseconds() as u64,
		900_000 - checkpoint.waited_millis
	);
}

#[rstest]
#[case::mcp("MCP_ELICITATION", false)]
#[case::question("QUESTION", true)]
#[tokio::test]
async fn resuming_human_input_journals_only_answers_intended_for_the_model(
	mut fixture: Fixture,
	#[case] kind: &str,
	#[case] journaled: bool,
) {
	let request = HumanRequest {
		id: Uuid::new_v4(),
		workspace_id: fixture.run.workspace_id,
		run_id: fixture.run.id,
		kind: kind.into(),
		prompt: "Server-only personal form".into(),
		response: Some(json!({"action":"accept","content":{"name":"Private name"}})),
		created_at: fixture.run.updated_at,
		answered_by: Some("alice".into()),
		answer_by: None,
	};
	fixture.run.state = RunState::Waiting(Box::new(WaitingState::Human {
		request_id: request.id,
		answer_by: None,
		resume: ResumeState::ToolCall(Box::new(ToolCallState {
			mcp: (kind == "MCP_ELICITATION").then(|| CallCheckpoint::new(KEY.into())),
			..Default::default()
		})),
	}));
	*fixture.backend.0.human.lock().unwrap() = Some(request);

	advance_sources(&mut fixture).await.unwrap();

	assert!(matches!(fixture.run.state, RunState::ToolCall(_)));
	assert_eq!(
		fixture
			.run
			.context
			.events()
			.any(|event| matches!(event, ContextEvent::Human { .. })),
		journaled
	);
	let (_, persisted) = last_write(&fixture);
	assert_eq!(
		persisted
			.context
			.events()
			.any(|event| matches!(event, ContextEvent::Human { .. })),
		journaled
	);
}
