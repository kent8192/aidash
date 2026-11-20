//! A Duty's Router or Watcher runs as a Run without a model (#1011): its Task
//! completes from the script call's stdout, and no step ever reaches Thinking.
use super::*;
use aidash_domain::duties::scripts::{Invocation, Language, TaskScript};

const MARK: &str = "\u{1e}aidash-duty-script-output\n";

fn tool(name: &str) -> Arc<Scripted> {
	Scripted::new(
		ToolSpec {
			name: name.into(),
			description: "Fixture".into(),
			parameters: json!({"type":"object"}),
		},
		name,
	)
}

/// A script Run whose shell runs once and whose poll answers with `polled`.
fn script_run(fixture: &mut Fixture, polled: Vec<Value>) -> (Arc<Scripted>, Arc<Scripted>) {
	let operation = Uuid::new_v4();
	let shell = tool("shell");
	*shell.outputs.lock().unwrap() = [json!({"status":"running", "operation_id":operation})].into();
	let poll = tool("shell_poll");
	*poll.outputs.lock().unwrap() = polled
		.into_iter()
		.map(|mut page| {
			page["operation_id"] = json!(operation);
			page
		})
		.collect();
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.custom.insert("shell".into(), shell.clone());
	state.custom.insert("shell_poll".into(), poll.clone());
	state.duty_script = TaskScript::Script(Invocation {
		language: Language::Shell,
		source: "echo '{\"action\":\"drop\"}'".into(),
		stdin: r#"{"event":{"pr":7},"trigger":{"id":"t","kind":"webhook"},"open_tasks":[]}"#.into(),
	});
	(shell, poll)
}

#[rstest]
#[tokio::test]
async fn a_duty_script_completes_its_task_from_the_calls_output_without_a_model(
	mut fixture: Fixture,
) {
	// Arrange: the command runs, then the poll returns its output in two pages.
	let (shell, poll) = script_run(
		&mut fixture,
		vec![
			json!({"status":"completed","result":{"exit_code":0,"output":format!("{MARK}{{\"action\":"),"next_offset":40}}),
			json!({"status":"completed","result":{"exit_code":0,"output":"\"drop\"}\n"}}),
		],
	);
	// Act: Ready, the call (which waits for its operation), the wake-up, the end.
	for _ in 0..4 {
		advance_sources(&mut fixture).await.unwrap();
	}
	// Assert
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(fixture.backend.task_value().status, TaskStatus::Completed);
	assert_eq!(
		*fixture.backend.0.artifacts.lock().unwrap(),
		vec![json!("{\"action\":\"drop\"}\n")]
	);
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
	let calls = fixture.backend.calls();
	assert!(!calls.contains(&"provider.infer"), "{calls:?}");
	assert!(
		!calls.contains(&"home.output"),
		"a script publishes no text"
	);
	assert_eq!(shell.invocations(), 1);
	assert_eq!(poll.invocations(), 2);
	// The command carries the script and its stdin, and its key is the Run's own.
	let RunState::Completed(_) = &fixture.run.state else {
		panic!("completed");
	};
	assert!(
		!fixture
			.run
			.context
			.events()
			.any(|event| matches!(event, ContextEvent::Tool { .. })),
		"a script Run records no Model Turn or tool result for a model"
	);
	assert_eq!(
		fixture.backend.events().first().map(String::as_str),
		Some("execution.started")
	);
}

#[rstest]
#[tokio::test]
async fn a_script_that_exits_unsuccessfully_fails_its_run_with_the_reason(mut fixture: Fixture) {
	script_run(
		&mut fixture,
		vec![
			json!({"status":"completed","result":{"exit_code":2,"output":format!(
				"{MARK}\n\u{1e}aidash-duty-script-error 2\nTraceback: KeyError 'pr'\n"
			)}}),
		],
	);
	for _ in 0..3 {
		advance_sources(&mut fixture).await.unwrap();
	}
	let error = advance_sources(&mut fixture).await.unwrap_err();
	assert!(
		error
			.to_string()
			.contains("exited with code 2: Traceback: KeyError 'pr'"),
		"{error}"
	);
	assert!(fixture.backend.0.artifacts.lock().unwrap().is_empty());
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_script_run_never_reaches_a_model(mut fixture: Fixture) {
	script_run(&mut fixture, vec![]);
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	let error = advance_sources(&mut fixture).await.unwrap_err();
	assert!(error.to_string().contains("never calls a model"), "{error}");
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn an_agent_without_the_scripts_tool_fails_with_the_reason(mut fixture: Fixture) {
	Arc::get_mut(&mut fixture.backend.0).unwrap().duty_script = TaskScript::Script(Invocation {
		language: Language::Python,
		source: "print('[]')".into(),
		stdin: String::new(),
	});
	let error = advance_sources(&mut fixture).await.unwrap_err();
	assert!(
		error.to_string().contains("cannot run python scripts"),
		"{error}"
	);
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
}

/// The input the bound `code_interpreter` accepts, as the server's `Python`
/// contract reads it: `code` is required and any other field is refused.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PythonInput {
	idempotency_key: Uuid,
	code: String,
	expected_revision: i64,
	expected_session_id: Option<Uuid>,
	timeout_seconds: Option<u64>,
	wait_seconds: Option<u64>,
}

/// A bound `code_interpreter` that refuses input the real operation refuses,
/// then answers as `inner` is scripted to.
struct PythonTool {
	inner: Arc<Scripted>,
	codes: Mutex<Vec<String>>,
}

#[async_trait]
impl ExecutionTool for PythonTool {
	fn mcp_rounds(&self) -> bool {
		false
	}
	fn specification(&self) -> ToolSpec {
		self.inner.specification()
	}
	fn contract(&self) -> aidash_domain::tool::ToolContract {
		self.inner.contract()
	}
	fn external_target(&self) -> Option<String> {
		None
	}
	fn replay_safe(&self) -> bool {
		self.inner.replay_safe()
	}
	async fn invoke(&self, run: &Run, input: Value, key: &str) -> Result<Value> {
		let input: PythonInput =
			serde_json::from_value(input).map_err(|error| Error::Invalid(error.to_string()))?;
		self.codes.lock().unwrap().push(input.code);
		self.inner.invoke(run, Value::Null, key).await
	}
}

/// A Python Router runs through `code_interpreter`, whose input names the
/// code `code` (#1011): the call is accepted and the Task completes.
#[rstest]
#[tokio::test]
async fn a_python_script_sends_its_code_as_the_python_operation_reads_it(mut fixture: Fixture) {
	// Arrange: the cell runs, then its poll returns the script's stdout.
	let operation = Uuid::new_v4();
	let cell = tool("code_interpreter");
	*cell.outputs.lock().unwrap() = [json!({"status":"running", "operation_id":operation})].into();
	let python = Arc::new(PythonTool {
		inner: cell,
		codes: Mutex::new(vec![]),
	});
	let poll = tool("python_poll");
	*poll.outputs.lock().unwrap() = [json!({
		"status":"completed",
		"operation_id":operation,
		"result":{"output":format!("{MARK}{{\"action\":\"drop\"}}\n")},
	})]
	.into();
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state
		.tools_by_alias
		.insert("code_interpreter".into(), python.clone());
	state.custom.insert("python_poll".into(), poll.clone());
	state.duty_script = TaskScript::Script(Invocation {
		language: Language::Python,
		source: "import json\nprint(json.dumps({'action': 'drop'}))\n".into(),
		stdin: r#"{"event":{"pr":7}}"#.into(),
	});
	// Act: Ready, the call (which waits for its operation), the wake-up, the end.
	for _ in 0..4 {
		advance_sources(&mut fixture).await.unwrap();
	}
	// Assert
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(fixture.backend.task_value().status, TaskStatus::Completed);
	assert_eq!(
		*fixture.backend.0.artifacts.lock().unwrap(),
		vec![json!("{\"action\":\"drop\"}\n")]
	);
	let codes = python.codes.lock().unwrap().clone();
	assert_eq!(codes.len(), 1, "{codes:?}");
	assert!(codes[0].contains("os.makedirs"), "{}", codes[0]);
	assert_eq!(poll.invocations(), 1);
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
}

/// A script Task whose script went with its Duty fails at its first step and
/// never asks a model (#1011).
#[rstest]
#[tokio::test]
async fn a_script_task_whose_script_is_gone_fails_before_it_starts(mut fixture: Fixture) {
	// Arrange
	Arc::get_mut(&mut fixture.backend.0).unwrap().duty_script = TaskScript::Gone;
	// Act
	let error = advance_sources(&mut fixture).await.unwrap_err();
	// Assert
	assert_eq!(error.to_string(), TaskScript::GONE);
	assert!(!matches!(fixture.run.state, RunState::Thinking(_)));
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
	assert!(
		!fixture
			.backend
			.events()
			.iter()
			.any(|event| event == "execution.started"),
		"{:?}",
		fixture.backend.events()
	);
	assert!(
		!fixture
			.run
			.context
			.events()
			.any(|event| matches!(event, ContextEvent::MemoryRecall { .. })),
		"a gone script recalls no memory"
	);
}

/// A script Run part way through its calls whose script went with its Duty
/// fails rather than run the rest as a model's calls, whether its Task still
/// shows it was a script Task or not (#1011).
#[rstest]
#[case::marked(TaskScript::Gone)]
#[case::unmarked(TaskScript::Model)]
#[tokio::test]
async fn a_script_run_whose_script_is_gone_mid_run_fails_without_a_call(
	mut fixture: Fixture,
	#[case] script: TaskScript,
) {
	// Arrange: the Run made its first call and has two left.
	let (shell, _) = script_run(&mut fixture, vec![]);
	Arc::get_mut(&mut fixture.backend.0).unwrap().duty_script = script;
	let tool_calls = (0..3)
		.map(|index| aidash_domain::provider::ToolCall {
			id: format!("{}{index}", aidash_domain::duties::scripts::CALL_PREFIX),
			name: "shell".into(),
			arguments: json!({"idempotency_key": Uuid::new_v4(), "command": "true", "expected_revision": 0}),
		})
		.collect();
	let mut state = ToolCallState {
		response: aidash_domain::provider::ModelResponse {
			tool_calls,
			usage_complete: true,
			..Default::default()
		},
		..Default::default()
	};
	state.advance(1);
	fixture.run.state = RunState::ToolCall(Box::new(state));
	// Act
	let error = advance_sources(&mut fixture).await.unwrap_err();
	// Assert
	assert_eq!(error.to_string(), TaskScript::GONE);
	assert_eq!(shell.invocations(), 0);
	assert!(fixture.backend.0.requests.lock().unwrap().is_empty());
	assert!(
		!fixture
			.run
			.context
			.events()
			.any(|event| matches!(event, ContextEvent::Tool { .. })),
		"no call is recorded as a model's"
	);
}
