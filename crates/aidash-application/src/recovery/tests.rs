use super::*;
use aidash_domain::{RecoveryState, Run, StateVersion, ThinkingState, context::Context};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
enum Write {
	Save(String),
	Pause(String, String),
	Semantic(SemanticFailure),
}
struct Repository {
	run: Mutex<Option<Run>>,
	writes: Mutex<Vec<Write>>,
	token: Uuid,
}
#[async_trait]
impl ExecutionRecoveryStore for Repository {
	async fn leased_run(&self, token: Uuid) -> Result<Option<Run>> {
		Ok((token == self.token)
			.then(|| self.run.lock().unwrap().clone())
			.flatten())
	}
	async fn save(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		assert_eq!(token, self.token);
		*self.run.lock().unwrap() = Some(run.clone());
		self.writes.lock().unwrap().push(Write::Save(event.into()));
		Ok(())
	}
	async fn pause(&self, run: &Run, token: Uuid, reason: &str, event: &str) -> Result<()> {
		assert_eq!(token, self.token);
		*self.run.lock().unwrap() = Some(run.clone());
		self.writes
			.lock()
			.unwrap()
			.push(Write::Pause(reason.into(), event.into()));
		Ok(())
	}
	async fn pause_semantic(&self, run: &Run, token: Uuid, reason: SemanticFailure) -> Result<()> {
		assert_eq!(token, self.token);
		*self.run.lock().unwrap() = Some(run.clone());
		self.writes.lock().unwrap().push(Write::Semantic(reason));
		Ok(())
	}
}
#[fixture]
fn now() -> DateTime<Utc> {
	"2026-10-02T00:00:00Z".parse().unwrap()
}
#[fixture]
fn repository(now: DateTime<Utc>) -> Repository {
	let token = Uuid::new_v4();
	Repository {
		token,
		writes: Mutex::new(vec![]),
		run: Mutex::new(Some(Run {
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			workspace_id: Uuid::new_v4(),
			home_node: "aidash://home".into(),
			agent_id: "fixture".into(),
			agent_version: "1.0.0".into(),
			state_version: StateVersion::default(),
			state: RunState::Thinking(ThinkingState::default()),
			recovery: RecoveryState::default(),
			control: RunControl::Active,
			context: Context::default(),
			step: 2,
			revision: 9,
			observed_input_seq: 7,
			ledger_worker_ready: true,
			error: None,
			lease_owner: Some(token),
			lease_until: Some(now + chrono::Duration::seconds(60)),
			updated_at: now,
		})),
	}
}
#[rstest]
#[tokio::test]
async fn pending_semantic_operations_do_not_consume_retry_allowance(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange: two prior unavailable attempts must survive an in-progress replay.
	repository
		.run
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.recovery
		.retry = Some(RetryState { count: 2, at: now });
	// Act
	let metric = recover(
		&repository,
		repository.token,
		ExecutionFailure::Semantic(SemanticFailure::Pending),
		now,
	)
	.await
	.unwrap();
	// Assert
	assert!(!metric);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	assert_eq!(run.recovery.retry.as_ref().unwrap().count, 2);
	assert_eq!(
		run.recovery.retry.as_ref().unwrap().at,
		now + chrono::Duration::seconds(1)
	);
	assert_eq!(run.observed_input_seq, 7);
	assert_eq!(run.revision, 9);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.semantic_retrying".into())]
	);
}
#[rstest]
#[tokio::test]
async fn exhausted_semantic_retries_require_manual_authority(
	repository: Repository,
	now: DateTime<Utc>,
) {
	repository
		.run
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.recovery
		.retry = Some(RetryState { count: 5, at: now });
	recover(
		&repository,
		repository.token,
		ExecutionFailure::Semantic(SemanticFailure::Unavailable),
		now,
	)
	.await
	.unwrap();
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Semantic(SemanticFailure::RetriesExhausted)]
	);
	assert_eq!(
		repository
			.run
			.lock()
			.unwrap()
			.as_ref()
			.unwrap()
			.recovery
			.semantic_reason,
		Some(SemanticFailure::RetriesExhausted)
	);
}
#[rstest]
#[case(408, true)]
#[case(429, true)]
#[case(500, true)]
#[case(503, true)]
#[case(400, false)]
#[case(401, false)]
#[tokio::test]
async fn provider_contract_and_transient_failures_have_distinct_recovery(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] status: u16,
	#[case] retries: bool,
) {
	let result = recover(
		&repository,
		repository.token,
		ExecutionFailure::Inference {
			transport: false,
			status: Some(status),
			message: "provider diagnostic".into(),
		},
		now,
	)
	.await
	.unwrap();
	assert_eq!(result, retries);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	if retries {
		assert_eq!(run.recovery.retry.as_ref().unwrap().count, 1);
		assert_eq!(
			run.recovery.retry.as_ref().unwrap().at,
			now + chrono::Duration::seconds(2)
		);
		assert!(matches!(run.state, RunState::Thinking(_)));
	} else {
		assert!(
			matches!(&run.state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::FailureDelivery { target: FailureTarget::Failed, .. }))
		);
	}
}
#[rstest]
#[case(false, "execution authority denied")]
#[case(true, "identity status unavailable")]
#[tokio::test]
async fn lost_authority_pauses_instead_of_replaying_work(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] identity_unavailable: bool,
	#[case] reason: &str,
) {
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Authority {
			identity_unavailable,
		},
		now,
	)
	.await
	.unwrap();
	assert!(!retry);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Pause(
			reason.into(),
			"run.authorization_blocked".into()
		)]
	);
}
#[rstest]
#[tokio::test]
async fn unacknowledged_terminal_delivery_remains_durable(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange: the tool failed earlier; only its Home acknowledgement is pending.
	repository.run.lock().unwrap().as_mut().unwrap().state =
		RunState::Waiting(Box::new(WaitingState::FailureDelivery {
			target: FailureTarget::Failed,
			wake_at: now,
			last_delivery_error: None,
		}));
	// Act
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Other("home unavailable".into()),
		now,
	)
	.await
	.unwrap();
	// Assert
	assert!(!retry);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	assert!(
		matches!(&run.state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::FailureDelivery { target: FailureTarget::Failed, wake_at, last_delivery_error: Some(error) } if *wake_at == now + chrono::Duration::seconds(5) && error == "home unavailable"))
	);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.failure_pending".into())]
	);
}
#[rstest]
#[tokio::test]
async fn a_new_lease_owner_prevents_any_recovery_write(repository: Repository, now: DateTime<Utc>) {
	let retry = recover(
		&repository,
		Uuid::new_v4(),
		ExecutionFailure::Other("late error".into()),
		now,
	)
	.await
	.unwrap();
	assert!(!retry);
	assert!(repository.writes.lock().unwrap().is_empty());
	assert!(matches!(
		repository.run.lock().unwrap().as_ref().unwrap().state,
		RunState::Thinking(_)
	));
}
