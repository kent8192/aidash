use super::*;
use aidash_domain::{RunMetadata, RunPhase, Task};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::Mutex;
use uuid::Uuid;

struct Scope {
	metadata: RunMetadata,
	calls: Vec<&'static str>,
	grant: bool,
	denied: bool,
	input_failed: bool,
	status: TaskStatus,
	finished: Option<TaskStatus>,
	paused: Option<bool>,
}
fn metadata() -> RunMetadata {
	RunMetadata {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		home_node: "fixture".into(),
		agent_id: "fixture".into(),
		agent_version: "1".into(),
		phase: RunPhase::Waiting,
		control: RunControl::Active,
		step: 1,
		revision: 2,
		observed_input_seq: 3,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
#[fixture]
fn scope() -> Scope {
	Scope {
		metadata: metadata(),
		calls: vec![],
		grant: false,
		denied: false,
		input_failed: false,
		status: TaskStatus::Running,
		finished: None,
		paused: None,
	}
}
fn task(scope: &Scope, status: TaskStatus) -> Task {
	Task {
		id: scope.metadata.task_id,
		workspace_id: scope.metadata.workspace_id,
		title: "fixture".into(),
		description: String::new(),
		status,
		requirements: serde_json::json!({}),
		owner: None,
		created_by: "fixture".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 2,
		created_at: scope.metadata.updated_at,
	}
}
#[async_trait]
impl FailureScope for Scope {
	fn metadata(&self) -> &RunMetadata {
		&self.metadata
	}
	fn target(&self) -> TaskStatus {
		TaskStatus::Failed
	}
	async fn remote_grant(&mut self) -> Result<bool> {
		self.calls.push("grant");
		Ok(self.grant)
	}
	async fn admit(&mut self) -> Result<()> {
		self.calls.push("admit");
		if self.denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn deliver_inputs(&mut self) -> Result<()> {
		self.calls.push("inputs");
		if self.input_failed {
			Err(Error::External("Home unavailable".into()))
		} else {
			Ok(())
		}
	}
	async fn task(&mut self) -> Result<Task> {
		self.calls.push("task");
		Ok(task(self, self.status))
	}
	async fn transition(&mut self, status: TaskStatus) -> Result<Task> {
		self.calls.push("transition");
		Ok(task(self, status))
	}
	async fn finish_authority(&mut self, result: Result<TaskStatus>) -> Result<TaskStatus> {
		self.calls.push("authority");
		result
	}
	async fn pause_authority(&mut self, identity_unavailable: bool) -> Result<()> {
		self.calls.push("pause");
		self.paused = Some(identity_unavailable);
		Ok(())
	}
	async fn finish(&mut self, result: Result<TaskStatus>) -> Result<()> {
		self.calls.push("finish");
		self.finished = Some(result?);
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn revoked_remote_cancellation_finishes_without_reaching_home(mut scope: Scope) {
	scope.metadata.control = RunControl::Cancelled;
	scope.grant = true;
	assert!(finish_cancelled(&mut scope).await.unwrap());
	assert_eq!(scope.finished, Some(TaskStatus::Cancelled));
	assert_eq!(scope.calls, ["grant", "finish"]);
}

#[rstest]
#[tokio::test]
async fn local_cancellation_still_requires_live_admission(mut scope: Scope) {
	scope.metadata.control = RunControl::Cancelled;
	assert!(!finish_cancelled(&mut scope).await.unwrap());
	assert_eq!(scope.finished, None);
	assert_eq!(scope.calls, ["grant"]);
}

#[rstest]
#[case(TaskStatus::Running, TaskStatus::Failed, true)]
#[case(TaskStatus::Completed, TaskStatus::Completed, false)]
#[case(TaskStatus::Cancelled, TaskStatus::Cancelled, false)]
#[tokio::test]
async fn terminal_home_outcome_is_never_overwritten(
	mut scope: Scope,
	#[case] current: TaskStatus,
	#[case] expected: TaskStatus,
	#[case] transition: bool,
) {
	scope.status = current;
	assert_eq!(deliver(&mut scope).await.unwrap(), expected);
	let expected_calls = if transition {
		vec!["admit", "inputs", "task", "transition", "authority"]
	} else {
		vec!["admit", "inputs", "task", "authority"]
	};
	assert_eq!(scope.calls, expected_calls);
}

#[rstest]
#[tokio::test]
async fn input_delivery_failure_finishes_authority_before_outbox_retry(mut scope: Scope) {
	scope.input_failed = true;
	assert!(
		matches!(deliver(&mut scope).await, Err(Error::External(message)) if message == "Home unavailable")
	);
	assert_eq!(scope.calls, ["admit", "inputs", "authority"]);
}

#[rstest]
#[tokio::test]
async fn denied_background_delivery_has_no_remote_or_task_effects(mut scope: Scope) {
	scope.denied = true;
	let result = deliver(&mut scope).await;
	settle(&mut scope, result).await.unwrap();
	assert_eq!(scope.calls, ["admit", "pause"]);
	assert_eq!(scope.paused, Some(false));
	assert_eq!(scope.finished, None);
}

#[rstest]
#[case(Error::Forbidden, false)]
#[case(Error::Unauthorized, false)]
#[case(Error::IdentityStatusUnavailable, true)]
#[tokio::test]
async fn lost_authority_pauses_instead_of_settling_failure(
	mut scope: Scope,
	#[case] error: Error,
	#[case] identity: bool,
) {
	settle(&mut scope, Err(error)).await.unwrap();
	assert_eq!(scope.paused, Some(identity));
	assert_eq!(scope.finished, None);
	assert_eq!(scope.calls, ["pause"]);
}

struct Repository {
	run: RunMetadata,
	calls: Mutex<Vec<&'static str>>,
}
#[async_trait]
impl TerminalRepository for Repository {
	async fn claim_failure(&self, _: Uuid, _: i32) -> Result<Option<Box<dyn FailureScope>>> {
		Ok(None)
	}
	async fn pending_inputs(&self) -> Result<Option<RunMetadata>> {
		Ok(Some(self.run.clone()))
	}
	async fn deliver_inputs(&self, run: &RunMetadata) -> Result<()> {
		assert_eq!(run.id, self.run.id);
		self.calls.lock().unwrap().push("deliver");
		Err(Error::External("Home unavailable".into()))
	}
	async fn defer_inputs(&self, run: Uuid) -> Result<()> {
		assert_eq!(run, self.run.id);
		self.calls.lock().unwrap().push("defer");
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn deferred_terminal_input_does_not_starve_runnable_work() {
	let repository = Repository {
		run: metadata(),
		calls: Mutex::new(vec![]),
	};
	assert!(!pending(&repository).await.unwrap());
	assert_eq!(*repository.calls.lock().unwrap(), ["deliver", "defer"]);
}
