use super::*;
use aidash_application::{Error, ports::ExecutionRecoveryStore, recovery::ExecutionFailure};
use aidash_domain::{
	RecoveryState, Run, RunControl, RunMetadata, RunState, StateVersion, ThinkingState,
	context::Context,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{
	collections::VecDeque,
	sync::{
		Mutex,
		atomic::{AtomicBool, Ordering},
	},
};
use tokio::time::Instant;

type Journal = Arc<Mutex<Vec<&'static str>>>;
struct Leases {
	trace: Journal,
	run: Uuid,
	renewals: Mutex<VecDeque<Result<bool>>>,
	controls: Mutex<VecDeque<Result<RunControl>>>,
	times: Mutex<Vec<Instant>>,
	blocked_renewal: Option<Arc<tokio::sync::Notify>>,
}
#[fixture]
fn leases() -> Leases {
	Leases {
		trace: Arc::new(Mutex::new(vec![])),
		run: Uuid::new_v4(),
		renewals: Mutex::new(VecDeque::new()),
		controls: Mutex::new(VecDeque::new()),
		times: Mutex::new(vec![]),
		blocked_renewal: None,
	}
}
#[async_trait]
impl WorkerLeases for Leases {
	async fn current_id(&self, _: Uuid) -> Result<Uuid> {
		self.trace.lock().unwrap().push("lookup");
		Err(Error::Conflict("worker lease lost".into()))
	}
	async fn renew(&self, run: Uuid, _: Uuid, _: i32) -> Result<bool> {
		assert_eq!(run, self.run);
		self.trace.lock().unwrap().push("renew");
		self.times.lock().unwrap().push(Instant::now());
		if let Some(entered) = &self.blocked_renewal {
			let _guard = PendingRenewal(self.trace.clone());
			entered.notify_one();
			return std::future::pending().await;
		}
		self.renewals
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or(Ok(true))
	}
	fn transient(&self, error: &Error) -> bool {
		matches!(error, Error::TransactionPending)
	}
	async fn control(&self, run: Uuid) -> Result<RunControl> {
		assert_eq!(run, self.run);
		self.trace.lock().unwrap().push("control");
		self.controls
			.lock()
			.unwrap()
			.pop_front()
			.unwrap_or(Ok(RunControl::Active))
	}
}

struct PendingRenewal(Journal);
impl Drop for PendingRenewal {
	fn drop(&mut self) {
		self.0.lock().unwrap().push("renew-cancelled");
	}
}

struct Scope {
	trace: Journal,
	run: Run,
	blocked: bool,
	resume_failed: bool,
}
impl Scope {
	fn new(leases: &Leases) -> Self {
		Self {
			trace: leases.trace.clone(),
			blocked: false,
			resume_failed: false,
			run: Run {
				id: leases.run,
				task_id: Uuid::new_v4(),
				workspace_id: Uuid::new_v4(),
				home_node: "fixture".into(),
				agent_id: "fixture".into(),
				agent_version: "1".into(),
				state_version: StateVersion::default(),
				state: RunState::Thinking(ThinkingState::default()),
				recovery: RecoveryState::default(),
				control: RunControl::Active,
				context: Context::default(),
				step: 0,
				revision: 1,
				observed_input_seq: 0,
				ledger_worker_ready: true,
				error: None,
				lease_owner: Some(Uuid::nil()),
				lease_until: None,
				updated_at: chrono::Utc::now(),
			},
		}
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		self.trace.lock().unwrap().push("drop");
	}
}
#[async_trait]
impl ExecutionRecoveryStore for Scope {
	async fn leased_run(&self, token: Uuid) -> Result<Option<Run>> {
		assert_eq!(token, Uuid::nil());
		self.trace.lock().unwrap().push("recover");
		Ok(Some(self.run.clone()))
	}
	async fn save(&self, _: &Run, _: Uuid, _: &str) -> Result<()> {
		panic!("authority failures must pause")
	}
	async fn pause(&self, run: &Run, token: Uuid, reason: &str, event: &str) -> Result<()> {
		assert_eq!(run.id, self.run.id);
		assert_eq!(token, Uuid::nil());
		assert_eq!(reason, "execution authority denied");
		assert_eq!(event, "run.authorization_blocked");
		self.trace.lock().unwrap().push("pause");
		Ok(())
	}
	async fn pause_semantic(
		&self,
		_: &Run,
		_: Uuid,
		_: aidash_domain::semantic::Failure,
	) -> Result<()> {
		panic!("authority failures must not become semantic failures")
	}
}
#[async_trait]
impl WorkerStep for Scope {
	fn metadata(&self) -> RunMetadata {
		self.run.metadata()
	}
	fn recovery_store(&self) -> &dyn ExecutionRecoveryStore {
		self
	}
	fn classify_failure(&self, error: Error) -> ExecutionFailure {
		assert!(matches!(error, Error::Forbidden));
		ExecutionFailure::Authority {
			identity_unavailable: false,
		}
	}
	async fn cancel_scoped(&mut self, _: Uuid) -> Result<bool> {
		self.trace.lock().unwrap().push("cancel");
		Ok(false)
	}
	async fn admit(&mut self) -> Result<()> {
		self.trace.lock().unwrap().push("admit");
		Ok(())
	}
	async fn invoke(&mut self, _: Uuid) -> Result<()> {
		self.trace.lock().unwrap().push("invoke");
		if self.blocked {
			std::future::pending().await
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn finish_authority(&mut self, result: Result<()>) -> Result<()> {
		self.trace.lock().unwrap().push("finish");
		result
	}
	async fn resume_visibility(&mut self) -> Result<()> {
		self.trace.lock().unwrap().push("resume");
		if self.resume_failed {
			Err(Error::TransactionPending)
		} else {
			Ok(())
		}
	}
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn transient_renewal_retries_use_capped_exponential_delay(leases: Leases) {
	// Arrange: transient visibility failures retain the original committed token.
	*leases.renewals.lock().unwrap() = (0..5)
		.map(|_| Err(Error::TransactionPending))
		.chain([Ok(true)])
		.collect();
	// Act
	assert!(renew(&leases, leases.run, Uuid::nil(), 60).await.unwrap());
	// Assert
	let times = leases.times.lock().unwrap();
	let delays: Vec<_> = times
		.windows(2)
		.map(|w| w[1].duration_since(w[0]))
		.collect();
	assert_eq!(
		delays,
		[
			Duration::from_millis(250),
			Duration::from_millis(500),
			Duration::from_secs(1),
			Duration::from_secs(2),
			Duration::from_secs(2)
		]
	);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn permanent_renewal_failure_is_not_retried(leases: Leases) {
	leases
		.renewals
		.lock()
		.unwrap()
		.push_back(Err(Error::Forbidden));
	assert!(matches!(
		renew(&leases, leases.run, Uuid::nil(), 60).await,
		Err(Error::Forbidden)
	));
	assert_eq!(*leases.trace.lock().unwrap(), ["renew"]);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn pending_renewal_does_not_stop_the_step_from_committing(mut leases: Leases) {
	// Arrange: the renewal cannot finish until the step releases its row lock.
	let entered = Arc::new(tokio::sync::Notify::new());
	leases.blocked_renewal = Some(entered.clone());
	let work = async {
		entered.notified().await;
		leases.trace.lock().unwrap().push("commit");
		Ok(77)
	};
	// Act: the committed result must finish without waiting for the renewal.
	let result = tokio::time::timeout(
		Duration::from_secs(2),
		keepalive(work, &leases, leases.run, Uuid::nil(), 3),
	)
	.await
	.expect("the step must progress while its renewal is pending")
	.unwrap();
	// Assert: completion also cancels the pending database operation.
	assert!(matches!(result, Completion::Completed(Ok(77))));
	assert_eq!(
		*leases.trace.lock().unwrap(),
		["renew", "commit", "renew-cancelled"]
	);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn released_step_owner_cancels_without_lookup_resume_or_recovery(leases: Leases) {
	let mut scope = Scope::new(&leases);
	scope.blocked = true;
	leases.renewals.lock().unwrap().push_back(Ok(false));
	advance(Box::new(scope), &leases, Uuid::nil(), 3)
		.await
		.unwrap();
	assert_eq!(
		*leases.trace.lock().unwrap(),
		["cancel", "admit", "invoke", "renew", "drop"]
	);
}

#[rstest]
#[tokio::test]
async fn recovery_reads_committed_state_after_visibility_resumes(leases: Leases) {
	advance(Box::new(Scope::new(&leases)), &leases, Uuid::nil(), 60)
		.await
		.unwrap();
	assert_eq!(
		*leases.trace.lock().unwrap(),
		[
			"cancel", "admit", "invoke", "finish", "resume", "recover", "pause", "drop"
		]
	);
}

#[rstest]
#[tokio::test]
async fn failed_visibility_resume_prevents_recovery(leases: Leases) {
	let mut scope = Scope::new(&leases);
	scope.resume_failed = true;
	assert!(matches!(
		advance(Box::new(scope), &leases, Uuid::nil(), 60).await,
		Err(Error::TransactionPending)
	));
	assert_eq!(
		*leases.trace.lock().unwrap(),
		["cancel", "admit", "invoke", "finish", "resume", "drop"]
	);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn committed_control_read_errors_do_not_cancel_inference(leases: Leases) {
	*leases.controls.lock().unwrap() = VecDeque::from([
		Err(Error::External("database offline".into())),
		Ok(RunControl::Active),
		Ok(RunControl::Paused),
		Ok(RunControl::Cancelled),
	]);
	let started = Instant::now();
	wait_for_cancellation(&leases, leases.run).await.unwrap();
	assert_eq!(started.elapsed(), Duration::from_millis(750));
	assert_eq!(
		*leases.trace.lock().unwrap(),
		["control", "control", "control", "control"]
	);
}

struct Probe(Arc<AtomicBool>);
impl Drop for Probe {
	fn drop(&mut self) {
		self.0.store(true, Ordering::Release);
	}
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn terminal_heartbeat_uses_claimed_run_and_cancels_lost_work(leases: Leases) {
	let dropped = Arc::new(AtomicBool::new(false));
	let probe = Probe(dropped.clone());
	let work = async move {
		let _probe = probe;
		std::future::pending::<Result<()>>().await
	};
	leases.renewals.lock().unwrap().push_back(Ok(false));
	assert!(matches!(
		keepalive(work, &leases, leases.run, Uuid::nil(), 3)
			.await
			.unwrap(),
		Completion::Lost
	));
	assert!(dropped.load(Ordering::Acquire));
	assert_eq!(*leases.trace.lock().unwrap(), ["renew"]);
}
