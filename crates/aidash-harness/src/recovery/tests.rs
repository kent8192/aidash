use super::*;
use aidash_application::{authorization::refusal_note, recovery::AuthorityFailure};
use aidash_domain::{
	RecoveryState, ResumeState, Run, StateVersion, ThinkingState,
	context::{Context, recovery::Failure as ContextFailure},
	tool::{AuthorityRefusal, RefusalCheck},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
enum Write {
	Save(String),
	Pause(String, String),
	Refused(String, String, AuthorityRefusal),
	Semantic(SemanticFailure),
	Context(ContextFailure),
}
struct Repository {
	run: Mutex<Option<Run>>,
	writes: Mutex<Vec<Write>>,
	token: Uuid,
	person: bool,
}
#[async_trait]
impl ExecutionRecoveryStore for Repository {
	async fn connection_wait_policy(
		&self,
		_run: &Run,
	) -> Result<Option<aidash_application::connection_wait::Policy>> {
		Ok(self
			.person
			.then_some(aidash_application::connection_wait::Policy {
				answer_deadline_seconds: Some(60),
				human_wait_seconds: 900,
			}))
	}
	async fn connection_request(
		&self,
		run: &Run,
		prompt: &str,
		_key: &str,
	) -> Result<aidash_domain::HumanRequest> {
		Ok(aidash_domain::HumanRequest {
			id: Uuid::now_v7(),
			run_id: run.id,
			workspace_id: run.workspace_id,
			kind: aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND.into(),
			prompt: prompt.into(),
			response: None,
			created_at: run.updated_at,
			answered_by: None,
			answer_by: None,
		})
	}

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
	async fn pause(
		&self,
		run: &Run,
		token: Uuid,
		reason: &str,
		event: &str,
		refusal: Option<&AuthorityRefusal>,
	) -> Result<()> {
		assert_eq!(token, self.token);
		*self.run.lock().unwrap() = Some(run.clone());
		self.writes.lock().unwrap().push(match refusal {
			Some(refusal) => Write::Refused(reason.into(), event.into(), refusal.clone()),
			None => Write::Pause(reason.into(), event.into()),
		});
		Ok(())
	}
	async fn pause_semantic(&self, run: &Run, token: Uuid, reason: SemanticFailure) -> Result<()> {
		assert_eq!(token, self.token);
		*self.run.lock().unwrap() = Some(run.clone());
		self.writes.lock().unwrap().push(Write::Semantic(reason));
		Ok(())
	}
	async fn pause_context(&self, run: &Run, token: Uuid, reason: ContextFailure) -> Result<()> {
		assert_eq!(token, self.token);
		*self.run.lock().unwrap() = Some(run.clone());
		self.writes.lock().unwrap().push(Write::Context(reason));
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
		person: false,
		token,
		writes: Mutex::new(vec![]),
		run: Mutex::new(Some(Run {
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
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
			control_revision: 0,
			control_source: Default::default(),
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
async fn a_repeated_pending_replay_backs_off_without_consuming_retry_allowance(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange: the previous replay already found the memory attempt live.
	{
		let mut run = repository.run.lock().unwrap();
		let recovery = &mut run.as_mut().unwrap().recovery;
		recovery.semantic_reason = Some(SemanticFailure::Pending);
		recovery.retry = Some(RetryState { count: 2, at: now });
	}
	// Act
	let metric = recover(
		&repository,
		repository.token,
		ExecutionFailure::Semantic(SemanticFailure::Pending),
		now,
	)
	.await
	.unwrap();
	// Assert: every replay repeats the charged workspace embedding, so the
	// next one waits longer, and the outage retry count stays the same.
	assert!(!metric);
	let run = repository.run.lock().unwrap();
	let retry = run.as_ref().unwrap().recovery.retry.as_ref().unwrap();
	assert_eq!(retry.count, 2);
	assert_eq!(retry.at, now + chrono::Duration::seconds(30));
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.semantic_retrying".into())]
	);
}
#[rstest]
#[tokio::test]
async fn exhausted_semantic_retries_enter_outage_wait(repository: Repository, now: DateTime<Utc>) {
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
		[Write::Save("run.outage_waiting".into())]
	);
	assert!(
		matches!(repository.run.lock().unwrap().as_ref().unwrap().state,
		RunState::Waiting(ref wait) if matches!(wait.as_ref(), WaitingState::Outage { service, wake_at, .. } if service == "Semantic context" && *wake_at == now + chrono::Duration::minutes(5)))
	);
	{
		// While the Run waits it retries by itself: it stays `Unavailable` with a
		// scheduled retry, so management offers only `wait` (ADR 0129).
		let run = repository.run.lock().unwrap();
		let recovery = &run.as_ref().unwrap().recovery;
		assert_eq!(recovery.semantic_reason, Some(SemanticFailure::Unavailable));
		// Only the wait's `wake_at` schedules the next check: the kept retry
		// carries no later time that would hold the wake-up back.
		let retry = recovery.retry.as_ref().unwrap();
		assert_eq!((retry.count, retry.at), (OUTAGE_RETRIES, now));
		assert_eq!(
			aidash_domain::semantic::recovery::Recovery::new(SemanticFailure::Unavailable, true)
				.actions,
			[aidash_domain::semantic::recovery::Action::Wait]
		);
	}
	// Only the bound pauses it as `RetriesExhausted`.
	recover(
		&repository,
		repository.token,
		ExecutionFailure::Semantic(SemanticFailure::Unavailable),
		now + aidash_domain::run_state::OUTAGE_BOUND,
	)
	.await
	.unwrap();
	assert_eq!(
		repository.writes.lock().unwrap().last(),
		Some(&Write::Semantic(SemanticFailure::RetriesExhausted))
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
#[case(1, None, 2)]
#[case(2, Some(12), 12)]
#[case(5, Some(90), 32)]
#[tokio::test]
async fn tool_retry_uses_invocation_count_and_bounded_retry_after(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] count: u32,
	#[case] retry_after: Option<u64>,
	#[case] seconds: i64,
) {
	assert!(
		recover(
			&repository,
			repository.token,
			ExecutionFailure::ToolRetry {
				count,
				retry_after: retry_after.map(std::time::Duration::from_secs),
				message: "not delivered".into()
			},
			now
		)
		.await
		.unwrap()
	);
	let run = repository.run.lock().unwrap();
	let retry = run.as_ref().unwrap().recovery.retry.as_ref().unwrap();
	assert_eq!(retry.count, count);
	assert_eq!(retry.at, now + chrono::Duration::seconds(seconds));
}

#[rstest]
#[tokio::test]
async fn a_cancelled_run_does_not_schedule_a_tool_retry(
	repository: Repository,
	now: DateTime<Utc>,
) {
	repository.run.lock().unwrap().as_mut().unwrap().control = RunControl::Cancelled;
	assert!(
		!recover(
			&repository,
			repository.token,
			ExecutionFailure::ToolRetry {
				count: 1,
				retry_after: None,
				message: "not delivered".into()
			},
			now
		)
		.await
		.unwrap()
	);
	let run = repository.run.lock().unwrap();
	assert!(
		matches!(&run.as_ref().unwrap().state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::FailureDelivery { target: FailureTarget::Cancelled, .. }))
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
#[case(
	AuthorityFailure::Denied(None),
	"model call: an authority check refused this step"
)]
#[case(AuthorityFailure::IdentityUnavailable, "identity status unavailable")]
#[tokio::test]
async fn lost_authority_pauses_instead_of_replaying_work(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] failure: AuthorityFailure,
	#[case] reason: &str,
) {
	let failure_kind = matches!(failure, AuthorityFailure::Denied(None));
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Authority(failure),
		now,
	)
	.await
	.unwrap();
	assert!(!retry);
	let writes = repository.writes.lock().unwrap();
	match (&failure_kind, writes.as_slice()) {
		(true, [Write::Refused(actual, event, refusal)]) => {
			assert_eq!(actual, reason);
			assert_eq!(event, "run.authorization_blocked");
			assert_eq!(refusal.check, RefusalCheck::Unnamed);
		}
		(false, [Write::Pause(actual, event)]) => {
			assert_eq!(actual, reason);
			assert_eq!(event, "run.authorization_blocked");
		}
		other => panic!("unexpected writes {other:?}"),
	}
}
#[rstest]
#[tokio::test]
async fn an_unnamed_refusal_takes_its_check_from_the_note_of_the_step(
	repository: Repository,
	now: DateTime<Utc>,
) {
	let retry = refusal_note::within(async {
		refusal_note::set(refusal_note::Note::new(
			RefusalCheck::Catalog,
			Some("catalog.use"),
		));
		recover(
			&repository,
			repository.token,
			ExecutionFailure::Authority(AuthorityFailure::Denied(None)),
			now,
		)
		.await
	})
	.await
	.unwrap();
	assert!(!retry);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Refused(
			"model call: Catalog Approval refused catalog.use".into(),
			"run.authorization_blocked".into(),
			AuthorityRefusal {
				tool: "model call".into(),
				check: RefusalCheck::Catalog,
				action: Some("catalog.use".into()),
				subject: None,
				call: None,
			}
		)]
	);
}

#[rstest]
#[tokio::test]
async fn a_refused_tool_pauses_naming_the_tool_and_the_check(
	repository: Repository,
	now: DateTime<Utc>,
) {
	let refusal = AuthorityRefusal {
		tool: "task_delegate".into(),
		check: RefusalCheck::Policy,
		action: Some("task.delegate".into()),
		subject: None,
		call: None,
	};
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Authority(AuthorityFailure::Denied(Some(refusal.clone()))),
		now,
	)
	.await
	.unwrap();
	assert!(!retry);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Refused(
			"task_delegate: task.delegate was denied".into(),
			"run.authorization_blocked".into(),
			refusal,
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
#[rstest]
#[tokio::test]
async fn a_stale_reused_retrieval_is_dropped_and_retrieved_again(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange: the Run reused a cached retrieval the Home no longer vouches for.
	repository
		.run
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.context
		.source_observation = Some(
		aidash_domain::context::sources::SourceObservation::new(
			"retrieval-key".into(),
			"bindings".into(),
			serde_json::json!({"memory":null,"semantic_memory":{}}),
		)
		.unwrap(),
	);
	// Act
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Semantic(SemanticFailure::Stale),
		now,
	)
	.await
	.unwrap();
	// Assert: the next attempt finds no cache, so it retrieves once more.
	assert!(!retry);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.semantic_retrying".into())]
	);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	assert!(run.context.source_observation.is_none());
	assert_eq!(run.recovery.semantic_reason, Some(SemanticFailure::Stale));
	let retry = run.recovery.retry.as_ref().unwrap();
	assert_eq!(
		(retry.count, retry.at),
		(1, now + chrono::Duration::seconds(1))
	);
}
#[rstest]
#[tokio::test]
async fn repeated_stale_retrievals_pause_after_the_bounded_retries(
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
		ExecutionFailure::Semantic(SemanticFailure::Stale),
		now,
	)
	.await
	.unwrap();
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Semantic(SemanticFailure::RetriesExhausted)]
	);
}
#[rstest]
#[case(ContextFailure::ContextUnreducible)]
#[case(ContextFailure::DeciderUnapproved)]
#[case(ContextFailure::DeciderCallsExhausted)]
#[case(ContextFailure::DeciderUnavailable)]
#[case(ContextFailure::SummaryUnavailable)]
#[case(ContextFailure::SummaryInvalid)]
#[case(ContextFailure::OverflowRetriesExhausted)]
#[case(ContextFailure::OutputTruncated)]
#[case(ContextFailure::Refused)]
#[tokio::test]
async fn typed_context_failures_pause_without_scheduling_a_retry(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] reason: ContextFailure,
) {
	// Arrange: even with retry allowance left, a context failure is final.
	repository
		.run
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.recovery
		.retry = Some(RetryState { count: 1, at: now });
	// Act
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Context(reason),
		now,
	)
	.await
	.unwrap();
	// Assert
	assert!(!retry);
	assert_eq!(*repository.writes.lock().unwrap(), [Write::Context(reason)]);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	assert_eq!(run.recovery.context_reason, Some(reason));
	assert_eq!(run.recovery.retry.as_ref().unwrap().count, 1);
	assert_eq!(run.recovery.retry.as_ref().unwrap().at, now);
	assert!(matches!(run.state, RunState::Thinking(_)));
	assert!(!ExecutionFailure::Context(reason).retryable());
	assert_eq!(
		ExecutionFailure::Context(reason).message(),
		reason.to_string()
	);
}
#[rstest]
#[tokio::test]
async fn a_decider_outage_retries_as_new_decisions_then_waits(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Each retry waits 2, 4, 8, 16 and 32 s and counts as the step's next
	// outage retry, which the next decision commits to (ADR 0066).
	for retries in 1..=5_u32 {
		let retry = recover(
			&repository,
			repository.token,
			ExecutionFailure::DeciderOutage,
			now,
		)
		.await
		.unwrap();
		assert!(!retry);
		let run = repository.run.lock().unwrap();
		let run = run.as_ref().unwrap();
		assert_eq!(run.recovery.decider_retry, retries);
		assert_eq!(
			run.recovery.retry.as_ref().unwrap().at,
			now + chrono::Duration::seconds(2_i64.pow(retries))
		);
		assert_eq!(run.recovery.context_reason, None);
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::DeciderOutage,
		now,
	)
	.await
	.unwrap();
	let writes = repository.writes.lock().unwrap();
	assert_eq!(writes.len(), 6);
	assert!(
		writes[..5]
			.iter()
			.all(|write| *write == Write::Save("run.decider_retrying".into()))
	);
	assert_eq!(writes[5], Write::Save("run.outage_waiting".into()));
	let run = repository.run.lock().unwrap();
	assert_eq!(
		run.as_ref().unwrap().recovery.context_reason,
		Some(ContextFailure::DeciderUnavailable)
	);
	assert!(!ExecutionFailure::DeciderOutage.retryable());
}

#[rstest]
#[tokio::test]
async fn decider_wait_preserves_start_and_pauses_after_twenty_four_hours(
	repository: Repository,
	now: DateTime<Utc>,
) {
	for _ in 0..6 {
		recover(
			&repository,
			repository.token,
			ExecutionFailure::DeciderOutage,
			now,
		)
		.await
		.unwrap();
	}
	{
		let mut saved = repository.run.lock().unwrap();
		let run = saved.as_mut().unwrap();
		let RunState::Waiting(wait) = run.state.clone() else {
			panic!("expected outage wait")
		};
		let WaitingState::Outage {
			started_at,
			wake_at,
			resume,
			..
		} = *wait
		else {
			panic!("expected outage")
		};
		assert_eq!(started_at, now);
		assert_eq!(wake_at, now + chrono::Duration::minutes(5));
		run.state = resume.into_state();
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::DeciderOutage,
		now + chrono::Duration::minutes(5),
	)
	.await
	.unwrap();
	{
		let mut saved = repository.run.lock().unwrap();
		let run = saved.as_mut().unwrap();
		let RunState::Waiting(wait) = run.state.clone() else {
			panic!("expected another wait")
		};
		let WaitingState::Outage {
			started_at,
			wake_at,
			resume,
			..
		} = *wait
		else {
			panic!("expected outage")
		};
		assert_eq!(started_at, now);
		assert_eq!(wake_at, now + chrono::Duration::minutes(10));
		run.state = resume.into_state();
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::DeciderOutage,
		now + chrono::Duration::hours(24),
	)
	.await
	.unwrap();
	assert_eq!(
		repository.writes.lock().unwrap().last(),
		Some(&Write::Context(ContextFailure::DeciderUnavailable))
	);
}
#[rstest]
#[tokio::test]
async fn a_node_that_did_not_answer_is_retried_then_waits_naming_it(
	repository: Repository,
	now: DateTime<Utc>,
) {
	let failure = || ExecutionFailure::NodeUnreachable {
		node: "aidash://other/sibling".into(),
		retry: true,
	};
	for retries in 1..=5_u32 {
		let retry = recover(&repository, repository.token, failure(), now)
			.await
			.unwrap();
		assert!(!retry);
		let run = repository.run.lock().unwrap();
		let run = run.as_ref().unwrap();
		assert_eq!(run.recovery.node_retry, retries);
		assert_eq!(
			run.recovery.retry.as_ref().unwrap().at,
			now + chrono::Duration::seconds(2_i64.pow(retries))
		);
	}
	recover(&repository, repository.token, failure(), now)
		.await
		.unwrap();
	let writes = repository.writes.lock().unwrap();
	assert_eq!(writes.len(), 6);
	assert!(
		writes[..5]
			.iter()
			.all(|write| *write == Write::Save("run.node_retrying".into()))
	);
	// Not answering is not a refusal: the Run waits and names the Node (ADR 0129).
	assert_eq!(writes[5], Write::Save("run.outage_waiting".into()));
	let run = repository.run.lock().unwrap();
	assert!(matches!(
		&run.as_ref().unwrap().state,
		RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Outage { service, wake_at, .. }
			if service == "Node aidash://other/sibling" && *wake_at == now + chrono::Duration::minutes(5))
	));
	assert!(!failure().retryable());
}

#[rstest]
#[tokio::test]
async fn a_node_that_did_not_answer_after_a_tools_effect_is_retried_like_any_other(
	repository: Repository,
	now: DateTime<Utc>,
) {
	recover(
		&repository,
		repository.token,
		ExecutionFailure::NodeUnreachable {
			node: "aidash://other/sibling".into(),
			retry: false,
		},
		now,
	)
	.await
	.unwrap();
	// Taking the step up again reconciles the STARTED call; it does not repeat it.
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.node_retrying".into())]
	);
}

#[rstest]
#[tokio::test]
async fn a_node_outage_pauses_once_its_wait_reaches_the_bound(
	repository: Repository,
	now: DateTime<Utc>,
) {
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		run.recovery.node_retry = 5;
		run.recovery.outage = Some(aidash_domain::run_state::OutageTracking {
			service: "Node aidash://other/sibling".into(),
			started_at: now - aidash_domain::run_state::OUTAGE_BOUND,
		});
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::NodeUnreachable {
			node: "aidash://other/sibling".into(),
			retry: true,
		},
		now,
	)
	.await
	.unwrap();
	let writes = repository.writes.lock().unwrap();
	let [Write::Pause(reason, event)] = writes.as_slice() else {
		panic!("the bound pauses: {writes:?}")
	};
	assert!(reason.contains("aidash://other/sibling"), "{reason}");
	assert_eq!(event, "run.node_unreachable");
}

#[rstest]
#[tokio::test]
async fn a_reached_plan_usage_limit_waits_an_hour_instead_of_failing(
	repository: Repository,
	now: DateTime<Utc>,
) {
	let failure = || ExecutionFailure::PlanAllowance("usage limit reached".into());
	recover(&repository, repository.token, failure(), now)
		.await
		.unwrap();
	{
		let writes = repository.writes.lock().unwrap();
		assert_eq!(*writes, [Write::Save("run.outage_waiting".into())]);
		let run = repository.run.lock().unwrap();
		assert!(matches!(
			&run.as_ref().unwrap().state,
			RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Outage { wake_at, .. }
				if *wake_at == now + chrono::Duration::hours(1))
		));
	}
	// After 24 hours the wait becomes a pause a person lifts (ADR 0129).
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		let RunState::Waiting(wait) = run.state.clone() else {
			panic!("expected a wait")
		};
		let WaitingState::Outage { resume, .. } = *wait else {
			panic!("expected an Outage Wait")
		};
		run.state = resume.into_state();
	}
	recover(
		&repository,
		repository.token,
		failure(),
		now + aidash_domain::run_state::OUTAGE_BOUND,
	)
	.await
	.unwrap();
	let writes = repository.writes.lock().unwrap();
	assert!(
		matches!(writes.last(), Some(Write::Pause(_, event)) if event == "run.plan_allowance_exhausted"),
		"{writes:?}"
	);
}

#[rstest]
#[tokio::test]
async fn closed_native_memory_is_retried_then_waited_out(
	repository: Repository,
	now: DateTime<Utc>,
) {
	let failure = || ExecutionFailure::ServiceOutage("Native memory".into());
	for _ in 0..5 {
		assert!(
			recover(&repository, repository.token, failure(), now)
				.await
				.unwrap()
		);
	}
	assert!(
		!recover(&repository, repository.token, failure(), now)
			.await
			.unwrap()
	);
	let writes = repository.writes.lock().unwrap();
	assert!(
		writes[..5]
			.iter()
			.all(|write| *write == Write::Save("run.retrying".into()))
	);
	assert_eq!(writes[5], Write::Save("run.outage_waiting".into()));
	let run = repository.run.lock().unwrap();
	assert!(matches!(
		&run.as_ref().unwrap().state,
		RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Outage { service, .. }
			if service == "Native memory")
	));
	// The wait keeps the spent short retries for its wake-up to restore, so a
	// failure after the wake waits again at once (ADR 0129).
	assert_eq!(
		run.as_ref()
			.unwrap()
			.recovery
			.retry
			.as_ref()
			.map(|retry| retry.count),
		Some(OUTAGE_RETRIES)
	);
}

#[rstest]
#[tokio::test]
async fn a_service_outage_after_its_wake_waits_again_without_more_short_retries(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// The step the wake-up resumed, with the count the wait kept restored.
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		run.recovery.retry = Some(RetryState {
			count: OUTAGE_RETRIES,
			at: now,
		});
		run.recovery.outage = Some(aidash_domain::run_state::OutageTracking {
			service: "Native memory".into(),
			started_at: now - chrono::Duration::minutes(5),
		});
	}
	let later = now + chrono::Duration::minutes(5);
	assert!(
		!recover(
			&repository,
			repository.token,
			ExecutionFailure::ServiceOutage("Native memory".into()),
			later,
		)
		.await
		.unwrap()
	);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.outage_waiting".into())]
	);
	let run = repository.run.lock().unwrap();
	assert!(matches!(
		&run.as_ref().unwrap().state,
		RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Outage { started_at, wake_at, .. }
			if *started_at == now - chrono::Duration::minutes(5) && *wake_at == later + chrono::Duration::minutes(5))
	));
}

#[rstest]
#[tokio::test]
async fn a_decider_or_node_wait_keeps_no_generic_retry_count(
	repository: Repository,
	now: DateTime<Utc>,
) {
	for _ in 0..6 {
		recover(
			&repository,
			repository.token,
			ExecutionFailure::DeciderOutage,
			now,
		)
		.await
		.unwrap();
	}
	// Its own count carries it; a generic count restored at the wake-up would
	// cut the retries of an unrelated failure on the woken step.
	let run = repository.run.lock().unwrap();
	let recovery = &run.as_ref().unwrap().recovery;
	assert_eq!(recovery.decider_retry, OUTAGE_RETRIES + 1);
	assert!(recovery.retry.is_none(), "{recovery:?}");
}

/// A check at worker entry (the Run-visibility check) runs before a woken wait
/// resumes its step, so it can fail while the stored state is still `Waiting`.
#[rstest]
#[tokio::test]
async fn an_outage_found_as_its_outage_wait_wakes_moves_the_next_check(
	repository: Repository,
	now: DateTime<Utc>,
) {
	let failure = || ExecutionFailure::NodeUnreachable {
		node: "aidash://other/sibling".into(),
		retry: true,
	};
	for _ in 0..6 {
		recover(&repository, repository.token, failure(), now)
			.await
			.unwrap();
	}
	// The wait wakes and the same Node fails worker entry; the stored state is
	// not taken back to the step first.
	let wake = now + chrono::Duration::minutes(5);
	recover(&repository, repository.token, failure(), wake)
		.await
		.unwrap();
	{
		let run = repository.run.lock().unwrap();
		let RunState::Waiting(wait) = &run.as_ref().unwrap().state else {
			panic!("the Run keeps waiting")
		};
		let WaitingState::Outage {
			service,
			started_at,
			wake_at,
			resume,
			..
		} = wait.as_ref()
		else {
			panic!("an Outage Wait: {wait:?}")
		};
		assert_eq!(service, "Node aidash://other/sibling");
		assert_eq!(*started_at, now);
		assert_eq!(*wake_at, wake + chrono::Duration::minutes(5));
		assert!(matches!(resume, ResumeState::Thinking(_)), "{resume:?}");
	}
	assert_eq!(
		repository.writes.lock().unwrap().last(),
		Some(&Write::Save("run.outage_waiting".into()))
	);
	// Another service failing at that wake-up is the one the wait now names.
	let decider = wake + chrono::Duration::minutes(5);
	for _ in 0..6 {
		recover(
			&repository,
			repository.token,
			ExecutionFailure::DeciderOutage,
			decider,
		)
		.await
		.unwrap();
	}
	assert!(matches!(
		&repository.run.lock().unwrap().as_ref().unwrap().state,
		RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::Outage { service, started_at, resume: ResumeState::Thinking(_), .. }
			if service == "Decider" && *started_at == decider)
	));
	// At the bound of that outage the Run pauses as before.
	recover(
		&repository,
		repository.token,
		ExecutionFailure::DeciderOutage,
		decider + aidash_domain::run_state::OUTAGE_BOUND,
	)
	.await
	.unwrap();
	assert_eq!(
		repository.writes.lock().unwrap().last(),
		Some(&Write::Context(ContextFailure::DeciderUnavailable))
	);
}

#[rstest]
#[case::timer(WaitingState::Timer {
	wake_at: "2026-10-02T00:00:00Z".parse().unwrap(),
	resume: ResumeState::Thinking(ThinkingState::default()),
})]
#[case::person(WaitingState::Human {
	request_id: Uuid::nil(),
	answer_by: None,
	resume: ResumeState::Thinking(ThinkingState::default()),
})]
#[case::dependencies(WaitingState::Dependencies {
	wake_at: "2026-10-02T00:00:00Z".parse().unwrap(),
	resume: Default::default(),
})]
#[tokio::test]
async fn an_outage_found_as_another_wait_wakes_keeps_that_wait(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] wait: WaitingState,
) {
	let kind = std::mem::discriminant(&wait);
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		run.state = RunState::Waiting(Box::new(wait));
		// The short retries are spent; this failure starts the Outage Wait.
		run.recovery.node_retry = OUTAGE_RETRIES;
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::NodeUnreachable {
			node: "aidash://other/sibling".into(),
			retry: true,
		},
		now,
	)
	.await
	.unwrap();
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.outage_waiting".into())]
	);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	// What the Run waits for stays; it is woken no earlier than the next check.
	assert!(
		matches!(&run.state, RunState::Waiting(stored) if std::mem::discriminant(stored.as_ref()) == kind),
		"{:?}",
		run.state
	);
	assert_eq!(
		run.recovery.retry.as_ref().map(|retry| retry.at),
		Some(now + chrono::Duration::minutes(5))
	);
	assert_eq!(
		run.recovery.outage.as_ref().map(|outage| outage.started_at),
		Some(now)
	);
}

#[rstest]
#[tokio::test]
async fn transport_failures_still_retry(repository: Repository, now: DateTime<Utc>) {
	let retry = recover(
		&repository,
		repository.token,
		ExecutionFailure::Inference {
			transport: true,
			status: None,
			message: "connection reset".into(),
		},
		now,
	)
	.await
	.unwrap();
	assert!(retry);
	assert_eq!(
		*repository.writes.lock().unwrap(),
		[Write::Save("run.retrying".into())]
	);
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	assert_eq!(run.recovery.retry.as_ref().unwrap().count, 1);
	assert_eq!(run.recovery.context_reason, None);
}

#[rstest]
#[case::first(true, false, false, true)]
#[case::already_waited(true, true, false, false)]
#[case::service(false, false, false, false)]
#[case::later_success(true, false, true, false)]
#[tokio::test]
async fn failure_defers_for_an_unsatisfied_connection_once_per_resource(
	mut repository: Repository,
	now: DateTime<Utc>,
	#[case] person: bool,
	#[case] waited: bool,
	#[case] succeeded: bool,
	#[case] defers: bool,
) {
	use aidash_domain::{context::ContextEvent, provider::ToolCall};
	use serde_json::json;
	let resource = "https://mcp.example";
	repository.person = person;
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		let call = ToolCall {
			id: "call".into(),
			name: "crm".into(),
			arguments: json!({}),
		};
		run.context.push(ContextEvent::tool(
			call.clone(),
			json!({"error":"connection_required","resource":resource}),
		));
		aidash_application::connection_wait::record_refusal(
			run,
			"crm",
			aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
				resource: resource.into(),
			},
			vec![],
		);
		if succeeded {
			run.context
				.push(ContextEvent::tool(call, json!({"ok":true})));
			aidash_application::connection_wait::record_success(run, "crm");
		}
		if waited {
			run.recovery.connection_waited.insert(resource.into());
		}
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::Other("task cannot continue".into()),
		now,
	)
	.await
	.unwrap();
	let run = repository.run.lock().unwrap();
	let run = run.as_ref().unwrap();
	assert_eq!(run.recovery.connection_wait.is_some(), defers);
	if defers {
		assert!(run.recovery.connection_waited.contains(resource));
		assert!(
			matches!(&run.state,RunState::Waiting(wait) if matches!(wait.as_ref(),WaitingState::Human {resume:aidash_domain::ResumeState::Thinking(_),..}))
		);
		assert_eq!(
			*repository.writes.lock().unwrap(),
			[Write::Save("run.failure_deferred_for_connection".into())]
		);
	} else {
		assert!(run.state.failure_delivery());
	}
}

#[rstest]
#[tokio::test]
async fn a_pin_time_connection_finding_can_defer_failure_without_any_tool_call(
	mut repository: Repository,
	now: DateTime<Utc>,
) {
	use aidash_domain::agent_plugin::*;
	repository.person = true;
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		run.context.plugin_tools = Some(PluginTools {
			servers: vec![],
			findings: vec![PluginFinding {
				plugin: "acme".to_owned().try_into().unwrap(),
				finding: Finding {
					component: Component::Server("docs".into()),
					outcome: Outcome::Skipped,
					reason: Reason::ServerUnavailable,
					detail: "connection_required".into(),
				},
				connection: Some(ConnectionCause::ConnectionRequired {
					resource: "https://mcp.example".into(),
				}),
			}],
		});
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::Other("no usable tools".into()),
		now,
	)
	.await
	.unwrap();
	let saved = repository.run.lock().unwrap().clone().unwrap();
	assert!(saved.recovery.connection_wait.is_some());
	// A resumed failure must not ask for the same resource again, even after compaction.
	{
		let mut run = repository.run.lock().unwrap();
		run.as_mut().unwrap().state = RunState::Thinking(Default::default());
		run.as_mut().unwrap().recovery.connection_wait = None;
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::Other("still cannot continue".into()),
		now,
	)
	.await
	.unwrap();
	assert!(
		repository
			.run
			.lock()
			.unwrap()
			.as_ref()
			.unwrap()
			.state
			.failure_delivery()
	);
}

#[rstest]
#[tokio::test]
async fn a_connection_shaped_tool_body_is_not_a_failure_deferral_candidate(
	mut repository: Repository,
	now: DateTime<Utc>,
) {
	repository.person = true;
	{
		let mut run = repository.run.lock().unwrap();
		let run = run.as_mut().unwrap();
		run.context.push(aidash_domain::context::ContextEvent::tool(
			aidash_domain::provider::ToolCall {
				id: "call".into(),
				name: "crm".into(),
				arguments: serde_json::json!({}),
			},
			serde_json::json!({"error":"connection_required","resource":"https://mcp.example"}),
		));
	}
	recover(
		&repository,
		repository.token,
		ExecutionFailure::Other("task cannot continue".into()),
		now,
	)
	.await
	.unwrap();
	assert!(
		repository
			.run
			.lock()
			.unwrap()
			.as_ref()
			.unwrap()
			.state
			.failure_delivery()
	);
}

#[rstest]
#[case("task_delegate", true)]
#[case("plugin__remote__ask", true)]
#[case("", false)]
#[case("two words", false)]
#[case("line\nbreak", false)]
#[case("<script>", false)]
#[case(&"x".repeat(65), false)]
fn only_a_plain_tool_name_is_shown_as_the_stage(#[case] name: &str, #[case] plain: bool) {
	assert_eq!(plain_tool_name(name), plain);
}

/// #728: a Run waiting on the database spends none of its attempts.
#[rstest]
#[tokio::test]
async fn waiting_on_the_database_spends_no_retry_allowance(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange
	{
		let mut run = repository.run.lock().unwrap();
		run.as_mut().unwrap().recovery.retry = Some(RetryState { count: 2, at: now });
	}
	// Act
	for _ in 0..8 {
		recover(
			&repository,
			repository.token,
			ExecutionFailure::DatabaseUnavailable,
			now,
		)
		.await
		.unwrap();
	}
	// Assert
	let run = repository.run.lock().unwrap();
	let retry = run.as_ref().unwrap().recovery.retry.as_ref().unwrap();
	assert_eq!(retry.count, 2);
	assert_eq!(retry.at, now + chrono::Duration::seconds(1));
}

/// #728: the Run shows why it waits, not the error of an earlier step.
#[rstest]
#[tokio::test]
async fn waiting_on_the_database_says_so(repository: Repository, now: DateTime<Utc>) {
	// Arrange
	{
		let mut run = repository.run.lock().unwrap();
		run.as_mut().unwrap().error = Some("an earlier step failed".into());
	}
	// Act
	recover(
		&repository,
		repository.token,
		ExecutionFailure::DatabaseUnavailable,
		now,
	)
	.await
	.unwrap();
	// Assert
	let run = repository.run.lock().unwrap();
	assert_eq!(
		run.as_ref().unwrap().error.as_deref(),
		Some("the database is temporarily unavailable")
	);
}
