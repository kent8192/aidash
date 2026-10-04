use super::*;
use crate::Error;
use aidash_domain::{RawRun, RunControl, RunState, TerminalState, context::Context};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::sync::Mutex;
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
#[fixture]
fn run() -> Run {
	serde_json::from_value::<RawRun>(json!({
		"id":id(1),"task_id":id(2),"workspace_id":id(3),"home_node":"aidash://home",
		"agent_id":"agent","agent_version":"1","phase":"READY","control":"CANCELLED",
		"step":4,"revision":7,"observed_input_seq":9,"ledger_worker_ready":true,
		"error":"old error","lease_owner":id(5),"lease_until":"2030-01-01T00:00:00Z",
		"updated_at":"2030-01-01T00:00:00Z","context":Context::default(),
		"pending":{"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":true}}
	}))
	.unwrap()
	.decode()
	.unwrap()
}
struct Repository {
	remote: bool,
	local: bool,
	fail: Option<&'static str>,
	calls: Mutex<Vec<String>>,
	saved: Mutex<Vec<Run>>,
}
#[fixture]
fn repository() -> Repository {
	Repository {
		remote: false,
		local: true,
		fail: None,
		calls: Mutex::new(vec![]),
		saved: Mutex::new(vec![]),
	}
}
impl Repository {
	fn record(&self, name: &str) -> Result<()> {
		self.calls.lock().unwrap().push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{name} fault"
			)))));
		}
		Ok(())
	}
}
#[async_trait]
impl ScopedCancellationRepository for Repository {
	async fn remote_grant(&self, run: &aidash_domain::RunMetadata) -> Result<bool> {
		assert_eq!(run.id, id(1));
		self.record("remote_grant")?;
		Ok(self.remote)
	}
	async fn local_grant(&self, run: &aidash_domain::RunMetadata) -> Result<bool> {
		assert_eq!(run.task_id, id(2));
		self.record("local_grant")?;
		Ok(self.local)
	}
	async fn save_receiver(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		assert_eq!(token, id(5));
		assert_eq!(event, "run.cancelled");
		self.record("save")?;
		self.saved.lock().unwrap().push(run.clone());
		Ok(())
	}
	async fn cancel_local(&self, run: &Run, token: Uuid) -> Result<()> {
		assert_eq!(token, id(5));
		self.record("cancel")?;
		self.saved.lock().unwrap().push(run.clone());
		Ok(())
	}
}
#[rstest]
#[case::active(RunControl::Active)]
#[case::paused(RunControl::Paused)]
#[tokio::test]
async fn noncancelled_runs_do_not_read_any_execution_grant(
	repository: Repository,
	mut run: Run,
	#[case] control: RunControl,
) {
	run.control = control;
	assert!(!cancel_if_scoped(&repository, &run, id(5)).await.unwrap());
	assert_eq!(*repository.calls.lock().unwrap(), Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn scoped_receiver_cleanup_clears_only_disposition_and_error_without_contacting_home(
	mut repository: Repository,
	run: Run,
) {
	repository.remote = true;
	assert!(cancel_if_scoped(&repository, &run, id(5)).await.unwrap());
	assert_eq!(
		*repository.calls.lock().unwrap(),
		vec!["remote_grant", "save"]
	);
	let saved = repository.saved.lock().unwrap();
	assert_eq!(saved.len(), 1);
	let mut expected = serde_json::to_value(&run).unwrap();
	expected["state"] = serde_json::to_value(RunState::Cancelled(TerminalState {})).unwrap();
	expected["error"] = Value::Null;
	assert_eq!(serde_json::to_value(&saved[0]).unwrap(), expected);
	assert_eq!(run.error.as_deref(), Some("old error"));
	assert_eq!(run.phase(), aidash_domain::RunPhase::Ready);
}
#[rstest]
#[tokio::test]
async fn local_scoped_cancellation_retains_the_original_run_for_its_atomic_cascade(
	repository: Repository,
	run: Run,
) {
	assert!(cancel_if_scoped(&repository, &run, id(5)).await.unwrap());
	assert_eq!(
		*repository.calls.lock().unwrap(),
		vec!["remote_grant", "local_grant", "cancel"]
	);
	assert_eq!(
		serde_json::to_value(&repository.saved.lock().unwrap()[0]).unwrap(),
		serde_json::to_value(run).unwrap()
	);
}
#[rstest]
#[tokio::test]
async fn legacy_runs_without_either_scoped_grant_leave_cancellation_to_the_existing_path(
	mut repository: Repository,
	run: Run,
) {
	repository.local = false;
	assert!(!cancel_if_scoped(&repository, &run, id(5)).await.unwrap());
	assert_eq!(
		*repository.calls.lock().unwrap(),
		vec!["remote_grant", "local_grant"]
	);
	assert!(repository.saved.lock().unwrap().is_empty());
}
#[rstest]
#[case::remote_grant("remote_grant", true)]
#[case::receiver_save("save", true)]
#[case::local_grant("local_grant", false)]
#[case::local_cancel("cancel", false)]
#[tokio::test]
async fn scoped_cancellation_faults_stop_at_the_existing_boundary(
	mut repository: Repository,
	run: Run,
	#[case] boundary: &'static str,
	#[case] remote: bool,
) {
	repository.remote = remote;
	repository.fail = Some(boundary);
	let Error::Port(error) = cancel_if_scoped(&repository, &run, id(5))
		.await
		.err()
		.unwrap()
	else {
		panic!("expected cancellation fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		format!("{boundary} fault")
	);
	assert_eq!(
		repository.calls.lock().unwrap().last().map(String::as_str),
		Some(boundary)
	);
	assert!(repository.saved.lock().unwrap().is_empty());
	if remote {
		assert!(
			!repository
				.calls
				.lock()
				.unwrap()
				.contains(&"local_grant".into())
		);
	}
}
