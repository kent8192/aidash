use super::*;
use aidash_domain::{ReadyState, RunMetadata, RunPhase, activation::Obligation, context::Context};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[fixture]
fn now() -> DateTime<Utc> {
	"2026-10-04T00:00:00Z".parse().unwrap()
}
fn raw(now: DateTime<Utc>) -> RawRun {
	RawRun {
		metadata: RunMetadata {
			id: Uuid::from_u128(1),
			task_id: Uuid::from_u128(2),
			workspace_id: Uuid::from_u128(3),
			home_node: "aidash://home".into(),
			agent_id: "fixture".into(),
			agent_version: "1.0.0".into(),
			phase: RunPhase::Ready,
			control: RunControl::Active,
			step: 0,
			revision: 7,
			observed_input_seq: 0,
			ledger_worker_ready: false,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: now,
		},
		context: serde_json::to_value(Context::default()).unwrap(),
		pending: aidash_domain::run_state::encode(
			&RunState::Ready(ReadyState {}),
			&Default::default(),
		)
		.unwrap(),
	}
}

#[rstest]
#[tokio::test]
async fn home_owned_human_waits_poll_without_querying_local_requests(
	now: DateTime<Utc>,
	repository: Repository,
) {
	let mut row = raw(now);
	row.metadata.phase = RunPhase::Waiting;
	row.context["binding_snapshot"] = serde_json::json!({"schema_version":1,"agent":{"registry_node":"aidash://receiver","id":"fixture","version":"1.0.0"},"remote":true,"bindings":[],"definitions":[]});
	row.pending = aidash_domain::run_state::encode(
		&RunState::Waiting(Box::new(WaitingState::Human {
			request_id: Uuid::new_v4(),
			resume: aidash_domain::ResumeState::Ready(ReadyState {}),
		})),
		&Default::default(),
	)
	.unwrap();
	let mut scope = Scope {
		state: repository.0.clone(),
		committed: false,
	};
	let polling = now + chrono::Duration::seconds(1);
	assert_eq!(
		due(&mut scope, &row, now, repository.node_id())
			.await
			.unwrap(),
		Some(polling)
	);
	assert_eq!(
		due(&mut scope, &row, polling, repository.node_id())
			.await
			.unwrap(),
		Some(polling)
	);
	assert!(
		!repository
			.0
			.events
			.lock()
			.unwrap()
			.iter()
			.any(|event| event == "answered")
	);
	row.context["binding_snapshot"]["remote"] = serde_json::json!(false);
	assert_eq!(
		due(&mut scope, &row, now, repository.node_id())
			.await
			.unwrap(),
		None
	);
	assert!(
		repository
			.0
			.events
			.lock()
			.unwrap()
			.iter()
			.any(|event| event == "answered")
	);
}

struct State {
	now: DateTime<Utc>,
	run: Mutex<Option<RawRun>>,
	rows: Mutex<Option<Vec<RawRun>>>,
	obligation: Mutex<Option<Obligation>>,
	publications: Mutex<Vec<Obligation>>,
	events: Mutex<Vec<String>>,
	visibility: AtomicUsize,
	suspended: AtomicBool,
	committed: AtomicBool,
	newer: AtomicBool,
	unblocked: AtomicBool,
	answered: AtomicBool,
	dependencies: AtomicBool,
	claimable: AtomicBool,
	repairable: AtomicBool,
	commit_error: AtomicBool,
	quarantine_error: AtomicBool,
}
impl State {
	fn record(&self, event: impl Into<String>) {
		self.events.lock().unwrap().push(event.into());
	}
}
#[derive(Clone)]
struct Repository(Arc<State>);
#[fixture]
fn repository(now: DateTime<Utc>) -> Repository {
	Repository(Arc::new(State {
		now,
		run: Mutex::new(Some(raw(now))),
		rows: Mutex::new(None),
		obligation: Mutex::new(Some(Obligation {
			id: Uuid::from_u128(4),
			generation: 8,
			run_id: Uuid::from_u128(1),
			run_revision: 7,
			state: "pending".into(),
			publication_epoch: 3,
		})),
		publications: Mutex::new(vec![]),
		events: Mutex::new(vec![]),
		visibility: AtomicUsize::new(0),
		suspended: AtomicBool::new(false),
		committed: AtomicBool::new(false),
		newer: AtomicBool::new(false),
		unblocked: AtomicBool::new(true),
		answered: AtomicBool::new(false),
		dependencies: AtomicBool::new(false),
		claimable: AtomicBool::new(true),
		repairable: AtomicBool::new(true),
		commit_error: AtomicBool::new(false),
		quarantine_error: AtomicBool::new(false),
	}))
}
struct Scope {
	state: Arc<State>,
	committed: bool,
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.committed {
			self.state.record("rollback");
		}
	}
}
#[async_trait]
impl SchedulingScope for Scope {
	async fn now(&mut self) -> Result<DateTime<Utc>> {
		self.state.record("now");
		Ok(self.state.now)
	}
	async fn candidates(&mut self, target: Option<Uuid>, _: RecoveryCursor) -> Result<Vec<RawRun>> {
		self.state.record(format!(
			"candidates:{}",
			target.map_or("all".into(), |id| id.to_string())
		));
		Ok(self
			.state
			.rows
			.lock()
			.unwrap()
			.clone()
			.unwrap_or_else(|| self.state.run.lock().unwrap().clone().into_iter().collect())
			.into_iter()
			.filter(|r| target.is_none_or(|id| r.id == id))
			.collect())
	}
	async fn unblocked(&mut self, _: Uuid) -> Result<bool> {
		self.state.record("unblocked");
		Ok(self.state.unblocked.load(Ordering::Acquire))
	}
	async fn human_answered(&mut self, _: Uuid) -> Result<bool> {
		self.state.record("answered");
		Ok(self.state.answered.load(Ordering::Acquire))
	}
	async fn approval(&mut self, _: Uuid) -> Result<Option<Approval>> {
		self.state.record("approval");
		Ok(None)
	}
	async fn dependencies_ready(&mut self, _: Uuid) -> Result<bool> {
		self.state.record("dependencies");
		Ok(self.state.dependencies.load(Ordering::Acquire))
	}
	async fn lease(&mut self, run: &Run, token: Uuid, seconds: i32) -> Result<Option<Run>> {
		self.state.record("lease");
		if !self.state.claimable.load(Ordering::Acquire) {
			return Ok(None);
		}
		let mut run = run.clone();
		run.lease_owner = Some(token);
		run.lease_until = Some(self.state.now + chrono::Duration::seconds(seconds.into()));
		run.revision += 1;
		Ok(Some(run))
	}
	async fn repair_lease(
		&mut self,
		run: &RunMetadata,
		_: Uuid,
		_: i32,
	) -> Result<Option<RunMetadata>> {
		self.state.record("repair_lease");
		Ok(self
			.state
			.repairable
			.load(Ordering::Acquire)
			.then(|| run.clone()))
	}
	async fn invalid_state(
		&mut self,
		_: &RunMetadata,
		_: Uuid,
		state: &RunState,
		reason: &str,
		node: &str,
	) -> Result<()> {
		assert_eq!(node, "aidash://home");
		assert_eq!(reason, "invalid execution context");
		assert!(
			matches!(state, RunState::Waiting(wait) if matches!(wait.as_ref(), WaitingState::FailureDelivery { target: FailureTarget::Failed, .. }))
		);
		self.state.record("invalid_state");
		Ok(())
	}
}
#[async_trait]
impl ClaimScope for Scope {
	async fn lock_run(&mut self, _: Uuid) -> Result<Option<RawRun>> {
		self.state.record("lock_run");
		Ok(self.state.run.lock().unwrap().clone())
	}
	async fn read_run(&mut self, _: Uuid) -> Result<RawRun> {
		self.state.record("read_run");
		Ok(self.state.run.lock().unwrap().clone().unwrap())
	}
	async fn lock_obligation(&mut self, _: Uuid) -> Result<Option<Obligation>> {
		self.state.record("lock_obligation");
		Ok(self.state.obligation.lock().unwrap().clone())
	}
	async fn newer_transition(&mut self, _: &Obligation, _: &RawRun) -> Result<bool> {
		self.state.record("newer");
		Ok(self.state.newer.load(Ordering::Acquire))
	}
	async fn settle(&mut self, _: Uuid, reason: &str) -> Result<()> {
		self.state.record(format!("settle:{reason}"));
		Ok(())
	}
	async fn record_claim(&mut self, _: Uuid, _: &Run, _: Uuid, _: i32) -> Result<()> {
		self.state.record("record_claim");
		Ok(())
	}
	async fn defer(&mut self, _: Uuid, _: Option<DateTime<Utc>>) -> Result<()> {
		self.state.record("defer");
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> Result<()> {
		self.state.record("commit");
		if self.state.commit_error.load(Ordering::Acquire) {
			return Err(crate::Error::TransactionPending);
		}
		self.committed = true;
		self.state.committed.store(true, Ordering::Release);
		Ok(())
	}
}
struct Visibility(Arc<State>);
impl Drop for Visibility {
	fn drop(&mut self) {
		self.0.visibility.fetch_sub(1, Ordering::AcqRel);
		self.0.record("visibility_drop");
	}
}
#[async_trait]
impl VisibilityScope for Visibility {
	async fn suspend(&mut self) -> Result<()> {
		self.0.record("suspend");
		self.0.suspended.store(true, Ordering::Release);
		Ok(())
	}
	async fn advance(self: Box<Self>, _: Run, _: Uuid) -> Result<()> {
		assert_eq!(self.0.visibility.load(Ordering::Acquire), 1);
		assert!(self.0.committed.load(Ordering::Acquire));
		self.0.record("advance");
		Ok(())
	}
}
#[async_trait]
impl ActivationRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://home"
	}
	fn lease_seconds(&self) -> i32 {
		30
	}
	async fn visibility(&self) -> Result<Box<dyn VisibilityScope>> {
		self.0.record("visibility");
		self.0.visibility.fetch_add(1, Ordering::AcqRel);
		Ok(Box::new(Visibility(self.0.clone())))
	}
	async fn claim_scope(&self) -> Result<Box<dyn ClaimScope>> {
		self.0.record("scope");
		Ok(Box::new(Scope {
			state: self.0.clone(),
			committed: false,
		}))
	}
	async fn recover(&self) -> Result<Option<(Run, Uuid)>> {
		self.0.record("recover");
		Ok(None)
	}
	async fn reconcile(&self) -> Result<u64> {
		Ok(0)
	}
	async fn publish_batch(&self, _: Uuid) -> Result<Vec<Obligation>> {
		self.0.record("publish_batch");
		Ok(self.0.publications.lock().unwrap().clone())
	}
	async fn published(&self, row: &Obligation, _: Uuid) -> Result<()> {
		self.0
			.record(format!("published:{}", row.publication_epoch));
		Ok(())
	}
	async fn quarantine(
		&self,
		_: &[u8],
		reason: QuarantineReason,
		sequence: Option<u64>,
	) -> Result<()> {
		assert_eq!(sequence, Some(11));
		self.0.record(format!("quarantine:{}", reason.as_str()));
		if self.0.quarantine_error.load(Ordering::Acquire) {
			Err(crate::Error::External("storage unavailable".into()))
		} else {
			Ok(())
		}
	}
	async fn observe(&self) -> Result<()> {
		Ok(())
	}
}
struct Delivery {
	state: Arc<State>,
	payload: Vec<u8>,
	ack_error: bool,
}
fn delivery(repository: &Repository) -> Delivery {
	Delivery {
		state: repository.0.clone(),
		payload: serde_json::to_vec(
			&repository
				.0
				.obligation
				.lock()
				.unwrap()
				.as_ref()
				.unwrap()
				.envelope(repository.node_id()),
		)
		.unwrap(),
		ack_error: false,
	}
}
#[async_trait]
impl ActivationDelivery for Delivery {
	fn payload(&self) -> &[u8] {
		&self.payload
	}
	fn sequence(&self) -> Option<u64> {
		Some(11)
	}
	async fn acknowledge(&self) -> Result<()> {
		assert!(self.state.committed.load(Ordering::Acquire));
		assert_eq!(self.state.visibility.load(Ordering::Acquire), 1);
		self.state.record("ack");
		if self.ack_error {
			Err(unavailable())
		} else {
			Ok(())
		}
	}
	async fn defer(&self) -> Result<()> {
		assert!(!self.state.committed.load(Ordering::Acquire));
		self.state.record("nak");
		Ok(())
	}
	async fn discard(&self) -> Result<()> {
		self.state.record("term");
		Ok(())
	}
}

#[rstest]
#[case::ack_success(false)]
#[case::ack_failure(true)]
#[tokio::test]
async fn committed_lease_advances_even_when_ack_fails(
	repository: Repository,
	#[case] ack_error: bool,
) {
	// Arrange
	let mut message = delivery(&repository);
	message.ack_error = ack_error;
	// Act
	let result = receive(&repository, &message, || repository.0.record("progress"))
		.await
		.unwrap();
	let (run, token, visibility) = result.claimed.unwrap();
	visibility.advance(*run, token).await.unwrap();
	// Assert
	assert_eq!(result.ack_failed, ack_error);
	assert!(result.progressed);
	let events = repository.0.events.lock().unwrap();
	let position = |event: &str| events.iter().position(|e| e == event).unwrap();
	assert!(position("lock_run") < position("lock_obligation"));
	assert!(position("lease") < position("record_claim"));
	assert!(position("record_claim") < position("commit"));
	assert!(position("commit") < position("progress"));
	assert!(position("progress") < position("ack"));
	assert!(position("ack") < position("advance"));
	assert!(position("advance") < position("visibility_drop"));
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 0);
}

#[rstest]
#[tokio::test]
async fn unknown_commit_is_nacked_without_ack_or_effect(repository: Repository) {
	// Arrange
	repository.0.commit_error.store(true, Ordering::Release);
	// Act
	let result = receive(&repository, &delivery(&repository), || {
		repository.0.record("progress")
	})
	.await;
	// Assert
	assert!(matches!(result, Err(crate::Error::TransactionPending)));
	let events = repository.0.events.lock().unwrap();
	assert!(events.iter().any(|e| e == "nak"));
	assert!(events.iter().any(|e| e == "rollback"));
	assert!(
		!events
			.iter()
			.any(|e| matches!(e.as_str(), "progress" | "ack" | "advance"))
	);
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 0);
}

#[rstest]
#[case::stored(false)]
#[case::storage_failure(true)]
#[tokio::test]
async fn quarantine_is_durable_before_term(repository: Repository, #[case] storage_failure: bool) {
	// Arrange
	let mut message = delivery(&repository);
	message.payload = b"not JSON".to_vec();
	repository
		.0
		.quarantine_error
		.store(storage_failure, Ordering::Release);
	// Act
	let result = receive(&repository, &message, || repository.0.record("progress")).await;
	// Assert
	assert_eq!(result.is_err(), storage_failure);
	let events = repository.0.events.lock().unwrap();
	assert_eq!(events[0], "quarantine:malformed");
	assert_eq!(events.iter().any(|e| e == "term"), !storage_failure);
	assert!(
		!events
			.iter()
			.any(|e| matches!(e.as_str(), "scope" | "visibility" | "ack"))
	);
}

#[rstest]
#[case::claimed("claimed")]
#[case::settled("settled")]
#[tokio::test]
async fn recorded_redelivery_never_releases_another_execution(
	repository: Repository,
	#[case] state: &str,
) {
	// Arrange
	repository
		.0
		.obligation
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.state = state.into();
	// Act
	let result = receive(&repository, &delivery(&repository), || {})
		.await
		.unwrap();
	// Assert
	assert!(result.claimed.is_none());
	assert!(result.progressed);
	let events = repository.0.events.lock().unwrap();
	assert!(events.iter().any(|e| e == "ack"));
	assert!(
		!events
			.iter()
			.any(|e| matches!(e.as_str(), "lease" | "advance" | "defer"))
	);
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 0);
}

#[rstest]
#[tokio::test]
async fn supersession_requires_a_newer_durable_obligation(repository: Repository) {
	// Arrange
	repository
		.0
		.obligation
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.run_revision = 6;
	repository.0.newer.store(true, Ordering::Release);
	// Act
	let envelope = repository
		.0
		.obligation
		.lock()
		.unwrap()
		.as_ref()
		.unwrap()
		.envelope(repository.node_id());
	let result = claim(&repository, &envelope).await.unwrap();
	// Assert
	assert!(matches!(result, Handoff::Recorded));
	let events = repository.0.events.lock().unwrap();
	assert!(events.iter().any(|e| e == "settle:newer_transition"));
	assert!(!events.iter().any(|e| e == "lease"));
}

#[rstest]
#[case::paused(RunControl::Paused, false, None)]
#[case::cancelled(RunControl::Cancelled, false, Some(0))]
#[case::live_owner(RunControl::Active, true, Some(60))]
#[tokio::test]
async fn ownership_and_control_precede_state_decoding(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] control: RunControl,
	#[case] owned: bool,
	#[case] expected: Option<i64>,
) {
	// Arrange
	let mut row = raw(now);
	row.metadata.control = control;
	row.context = serde_json::json!("broken");
	row.pending = serde_json::json!("broken");
	row.metadata.lease_until = owned.then_some(now + chrono::Duration::seconds(60));
	let mut scope = Scope {
		state: repository.0.clone(),
		committed: false,
	};
	// Act
	let result = due(&mut scope, &row, now, repository.node_id())
		.await
		.unwrap();
	// Assert
	assert_eq!(
		result,
		expected.map(|seconds| now + chrono::Duration::seconds(seconds))
	);
	assert!(repository.0.events.lock().unwrap().is_empty());
}

#[rstest]
#[case::fenced(true)]
#[case::lost_fence(false)]
#[tokio::test]
async fn malformed_context_requires_a_repair_lease_before_writes(
	repository: Repository,
	#[case] repairable: bool,
) {
	// Arrange
	repository.0.run.lock().unwrap().as_mut().unwrap().context = serde_json::json!(null);
	repository.0.repairable.store(repairable, Ordering::Release);
	let mut scope = Scope {
		state: repository.0.clone(),
		committed: false,
	};
	// Act
	let result = lease(
		&mut scope,
		Uuid::new_v4(),
		30,
		None,
		repository.node_id(),
		&mut None,
	)
	.await
	.unwrap();
	// Assert
	assert!(result.is_none());
	let events = repository.0.events.lock().unwrap();
	assert!(events.iter().any(|e| e == "repair_lease"));
	assert_eq!(events.iter().any(|e| e == "invalid_state"), repairable);
	assert!(!events.iter().any(|e| e == "lease"));
}

#[rstest]
#[case::full_batch(128, true)]
#[case::last_batch(127, false)]
#[tokio::test]
async fn blocked_recovery_rows_advance_the_keyset_cursor(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] count: usize,
	#[case] more: bool,
) {
	// Arrange
	let rows: Vec<_> = (1..=count)
		.map(|index| {
			let mut row = raw(now);
			row.metadata.id = Uuid::from_u128(index as u128);
			row
		})
		.collect();
	let last = rows.last().unwrap().metadata.id;
	*repository.0.rows.lock().unwrap() = Some(rows);
	repository.0.unblocked.store(false, Ordering::Release);
	let mut scope = Scope {
		state: repository.0.clone(),
		committed: false,
	};
	let mut cursor = None;
	// Act
	let result = lease(
		&mut scope,
		Uuid::new_v4(),
		30,
		None,
		repository.node_id(),
		&mut cursor,
	)
	.await
	.unwrap();
	// Assert
	assert!(result.is_none());
	assert_eq!(cursor, more.then_some((now, last)));
}

#[rstest]
#[tokio::test]
async fn failure_delivery_remains_outside_normal_leasing(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange
	let mut row = raw(now);
	row.metadata.phase = RunPhase::Waiting;
	row.context = serde_json::json!(null);
	row.pending = aidash_domain::run_state::encode(
		&RunState::Waiting(Box::new(WaitingState::FailureDelivery {
			target: FailureTarget::Failed,
			wake_at: now,
			last_delivery_error: None,
		})),
		&Default::default(),
	)
	.unwrap();
	*repository.0.run.lock().unwrap() = Some(row);
	let mut scope = Scope {
		state: repository.0.clone(),
		committed: false,
	};
	// Act
	let result = lease(
		&mut scope,
		Uuid::new_v4(),
		30,
		None,
		repository.node_id(),
		&mut None,
	)
	.await
	.unwrap();
	// Assert
	assert!(result.is_none());
	assert!(
		!repository
			.0
			.events
			.lock()
			.unwrap()
			.iter()
			.any(|e| matches!(e.as_str(), "repair_lease" | "lease"))
	);
}

struct SendGuard<'a>(&'a AtomicUsize);
impl Drop for SendGuard<'_> {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::AcqRel);
	}
}
struct Transport {
	state: Arc<State>,
	in_flight: AtomicUsize,
	peak: AtomicUsize,
	backpressure: bool,
}
#[async_trait]
impl ActivationTransport for Transport {
	fn disconnected(&self) -> bool {
		false
	}
	fn connected(&self) -> bool {
		true
	}
	async fn publish(&self, _: &Envelope, epoch: i64) -> Result<bool> {
		assert!(self.state.suspended.load(Ordering::Acquire));
		assert_eq!(self.state.visibility.load(Ordering::Acquire), 1);
		let active = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
		self.peak.fetch_max(active, Ordering::AcqRel);
		let _send = SendGuard(&self.in_flight);
		tokio::time::sleep(Duration::from_millis(1)).await;
		Ok(!self.backpressure || epoch != 1)
	}
	async fn fetch(&self) -> Result<Option<Box<dyn ActivationDelivery>>> {
		Err(crate::Error::Invalid("unexpected pull".into()))
	}
}

#[rstest]
#[tokio::test]
async fn backpressure_retains_unaccepted_rows_and_sends_at_most_eight(repository: Repository) {
	// Arrange
	let original = repository.0.obligation.lock().unwrap().clone().unwrap();
	*repository.0.publications.lock().unwrap() = (1..=20)
		.map(|epoch| Obligation {
			publication_epoch: epoch,
			..original.clone()
		})
		.collect();
	let transport = Transport {
		state: repository.0.clone(),
		in_flight: AtomicUsize::new(0),
		peak: AtomicUsize::new(0),
		backpressure: true,
	};
	// Act
	let result = publish(&repository, &transport).await.unwrap();
	// Assert
	assert!(matches!(result, Publication::Backpressured));
	assert_eq!(transport.peak.load(Ordering::Acquire), 8);
	let events = repository.0.events.lock().unwrap();
	assert_eq!(
		events
			.iter()
			.filter(|e| e.starts_with("published:"))
			.count(),
		19
	);
	assert!(!events.iter().any(|e| e == "published:1"));
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 0);
}
