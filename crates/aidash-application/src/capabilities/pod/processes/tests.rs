use super::*;
use crate::capabilities::pod::tests::{Runtime, context, reply};
use std::sync::Arc;

fn process(id: &str, run: &str) -> CommandResult {
	let mut process = reply("running");
	process.operation_id = id.into();
	process.run_id = Some(run.into());
	process.background = Some(true);
	process
}

fn runtime() -> Arc<Runtime> {
	let runtime = Arc::new(Runtime::default());
	*runtime.processes.lock().unwrap() = vec![
		process("mine.left", "run-a"),
		process("theirs.left", "run-b"),
	];
	runtime
}

#[tokio::test]
async fn a_run_lists_only_its_own_background_processes() {
	let runtime = runtime();
	let listed = list(&context(runtime), "run-a").await.unwrap();
	let processes = listed["processes"].as_array().unwrap();
	assert_eq!(processes.len(), 1);
	assert_eq!(processes[0]["operation_id"], "mine.left");
	assert_eq!(processes[0]["status"], "running");
}

#[tokio::test]
async fn a_run_cannot_stop_or_read_another_runs_process() {
	let runtime = runtime();
	let context = context(runtime.clone());
	let stop_error = stop(
		&context,
		"run-a",
		ProcessStop {
			operation_id: "theirs.left".into(),
		},
	)
	.await
	.unwrap_err();
	assert!(matches!(stop_error, Error::NotFound(_)));
	let log_error = log(
		&context,
		"run-a",
		ProcessLog {
			operation_id: "theirs.left".into(),
			stream: None,
			offset: None,
			limit: None,
		},
	)
	.await
	.unwrap_err();
	assert!(matches!(log_error, Error::NotFound(_)));
	assert!(runtime.stopped.lock().unwrap().is_empty());
	assert!(runtime.logs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stopping_an_own_process_reaches_the_runner() {
	let runtime = runtime();
	let stopped = stop(
		&context(runtime.clone()),
		"run-a",
		ProcessStop {
			operation_id: "mine.left".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(stopped["status"], "stopped");
	assert_eq!(*runtime.stopped.lock().unwrap(), ["mine.left"]);
}

#[tokio::test]
async fn a_log_read_is_bounded_and_decoded() {
	let runtime = runtime();
	let context = context(runtime.clone());
	let too_much = log(
		&context,
		"run-a",
		ProcessLog {
			operation_id: "mine.left".into(),
			stream: None,
			offset: None,
			limit: Some(LOG_BYTES + 1),
		},
	)
	.await;
	assert!(matches!(too_much, Err(Error::Invalid(_))));
	let read = log(
		&context,
		"run-a",
		ProcessLog {
			operation_id: "mine.left".into(),
			stream: Some("stderr".into()),
			offset: Some(4),
			limit: Some(100),
		},
	)
	.await
	.unwrap();
	assert_eq!(read["content"], "log line\n");
	let request = runtime.logs.lock().unwrap()[0].clone();
	assert_eq!(request.stream, "stderr");
	assert_eq!(request.offset, Some(4));
	assert_eq!(request.limit, 100);
}
