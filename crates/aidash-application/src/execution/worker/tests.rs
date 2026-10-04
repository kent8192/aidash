use super::*;
use crate::{Error, ports::ExecutionRecoveryStore, recovery::ExecutionFailure};
use aidash_domain::{Run, RunMetadata, RunPhase};
use async_trait::async_trait;
use rstest::{fixture, rstest};

struct Scope {
	calls: Vec<&'static str>,
	cancelled: bool,
	denied: bool,
	failed: bool,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		calls: vec![],
		cancelled: false,
		denied: false,
		failed: false,
	}
}
#[async_trait]
impl ExecutionRecoveryStore for Scope {
	async fn leased_run(&self, _: Uuid) -> Result<Option<Run>> {
		Ok(None)
	}
	async fn save(&self, _: &Run, _: Uuid, _: &str) -> Result<()> {
		panic!("admission must not recover a run")
	}
	async fn pause(&self, _: &Run, _: Uuid, _: &str, _: &str) -> Result<()> {
		panic!("admission must not recover a run")
	}
	async fn pause_semantic(
		&self,
		_: &Run,
		_: Uuid,
		_: aidash_domain::semantic::Failure,
	) -> Result<()> {
		panic!("admission must not recover a run")
	}
}
#[async_trait]
impl WorkerStep for Scope {
	fn metadata(&self) -> RunMetadata {
		RunMetadata {
			id: Uuid::nil(),
			task_id: Uuid::nil(),
			workspace_id: Uuid::nil(),
			home_node: "fixture".into(),
			agent_id: "fixture".into(),
			agent_version: "1".into(),
			phase: RunPhase::Thinking,
			control: RunControl::Active,
			step: 0,
			revision: 0,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: chrono::Utc::now(),
		}
	}
	fn recovery_store(&self) -> &dyn ExecutionRecoveryStore {
		self
	}
	fn classify_failure(&self, error: Error) -> ExecutionFailure {
		ExecutionFailure::Other(error.to_string())
	}
	async fn cancel_scoped(&mut self, _: Uuid) -> Result<bool> {
		self.calls.push("cancel");
		Ok(self.cancelled)
	}
	async fn admit(&mut self) -> Result<()> {
		self.calls.push("admit");
		if self.denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn invoke(&mut self, _: Uuid) -> Result<()> {
		self.calls.push("invoke");
		if self.failed {
			Err(Error::Conflict("stale response".into()))
		} else {
			Ok(())
		}
	}
	async fn finish_authority(&mut self, result: Result<()>) -> Result<()> {
		self.calls.push("finish");
		result
	}
	async fn resume_visibility(&mut self) -> Result<()> {
		panic!("admission does not own visibility supervision")
	}
}

#[rstest]
#[tokio::test]
async fn scoped_cancellation_precedes_live_admission(mut scope: Scope) {
	// Arrange: revoked receiver cancellation can finish without contacting Home.
	scope.cancelled = true;
	// Act
	advance(&mut scope, Uuid::nil()).await.unwrap();
	// Assert
	assert_eq!(scope.calls, ["cancel"]);
}

#[rstest]
#[tokio::test]
async fn denied_worker_never_invokes_an_agent(mut scope: Scope) {
	scope.denied = true;
	let result = advance(&mut scope, Uuid::nil()).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, ["cancel", "admit"]);
}

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn authority_boundary_receives_the_original_agent_outcome(
	mut scope: Scope,
	#[case] failed: bool,
) {
	scope.failed = failed;
	let result = advance(&mut scope, Uuid::nil()).await;
	if failed {
		assert!(matches!(result, Err(Error::Conflict(message)) if message == "stale response"));
	} else {
		result.unwrap();
	}
	assert_eq!(scope.calls, ["cancel", "admit", "invoke", "finish"]);
}
