//! A Duty's Router or Watcher runs as a Run without a model (#1011): its Task
//! completes from the script call's stdout, and no step ever reaches Thinking.
use super::*;
use aidash_domain::duties::scripts::{Invocation, Language};

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
	state.duty_script = Some(Invocation {
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
		Some("run.started")
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
	Arc::get_mut(&mut fixture.backend.0).unwrap().duty_script = Some(Invocation {
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
