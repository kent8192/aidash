use super::*;
use aidash_domain::{
	RecoveryState, Run, StateVersion, ThinkingState,
	context::{Context, recovery::Failure as ContextFailure},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
enum Write {
	Save(String),
	Pause(String, String),
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
async fn a_decider_outage_retries_as_new_decisions_then_pauses(
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
	assert_eq!(
		writes[5],
		Write::Context(ContextFailure::DeciderUnavailable)
	);
	let run = repository.run.lock().unwrap();
	assert_eq!(
		run.as_ref().unwrap().recovery.context_reason,
		Some(ContextFailure::DeciderUnavailable)
	);
	assert!(!ExecutionFailure::DeciderOutage.retryable());
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
