use super::*;
use aidash_application::Error;
use aidash_domain::{
	ReadyState, Run, RunControl, RunState, StateVersion,
	activation::{Envelope, Obligation, QuarantineReason},
	context::Context,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::{Mutex, atomic::AtomicUsize};
use tokio::{sync::Notify, task::JoinSet};
use uuid::Uuid;

struct State {
	run: Mutex<Option<Run>>,
	recoveries: AtomicUsize,
	visibility: AtomicUsize,
	pulls: AtomicUsize,
	peak_pulls: AtomicUsize,
	entered: Notify,
	release: Notify,
	connecting: Notify,
}
#[derive(Clone)]
struct Repository(Arc<State>);
#[fixture]
fn repository() -> Repository {
	let now = "2026-10-04T00:00:00Z".parse().unwrap();
	Repository(Arc::new(State {
		run: Mutex::new(Some(Run {
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
			id: Uuid::new_v4(),
			task_id: Uuid::new_v4(),
			workspace_id: Uuid::new_v4(),
			home_node: "aidash://home".into(),
			agent_id: "fixture".into(),
			agent_version: "1.0.0".into(),
			state_version: StateVersion::default(),
			state: RunState::Ready(ReadyState {}),
			recovery: Default::default(),
			control: RunControl::Active,
			context: Context::default(),
			step: 0,
			revision: 1,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			control_revision: 0,
			control_source: Default::default(),
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: now,
		})),
		recoveries: AtomicUsize::new(0),
		visibility: AtomicUsize::new(0),
		pulls: AtomicUsize::new(0),
		peak_pulls: AtomicUsize::new(0),
		entered: Notify::new(),
		release: Notify::new(),
		connecting: Notify::new(),
	}))
}
struct Visibility(Arc<State>);
impl Drop for Visibility {
	fn drop(&mut self) {
		self.0.visibility.fetch_sub(1, Ordering::AcqRel);
	}
}
#[async_trait]
impl VisibilityScope for Visibility {
	async fn suspend(&mut self) -> Result<()> {
		Ok(())
	}
	async fn advance(self: Box<Self>, _: Run, _: Uuid) -> Result<()> {
		self.0.entered.notify_one();
		self.0.release.notified().await;
		Ok(())
	}
}
fn unexpected() -> Error {
	Error::Invalid("unexpected activation test operation".into())
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
		self.0.visibility.fetch_add(1, Ordering::AcqRel);
		Ok(Box::new(Visibility(self.0.clone())))
	}
	async fn claim_scope(&self) -> Result<Box<dyn ClaimScope>> {
		Err(unexpected())
	}
	async fn recover(&self) -> Result<Option<(Run, Uuid)>> {
		// A recovery claim must not overlap a fetch from the same finite slot.
		assert_eq!(self.0.pulls.load(Ordering::Acquire), 0);
		self.0.recoveries.fetch_add(1, Ordering::AcqRel);
		Ok(self
			.0
			.run
			.lock()
			.unwrap()
			.take()
			.map(|run| (run, Uuid::new_v4())))
	}
	async fn reconcile(&self) -> Result<u64> {
		Ok(0)
	}
	async fn publish_batch(&self, _: Uuid) -> Result<Vec<Obligation>> {
		Err(unexpected())
	}
	async fn published(&self, _: &Obligation, _: Uuid) -> Result<()> {
		Err(unexpected())
	}
	async fn quarantine(&self, _: &[u8], _: QuarantineReason, _: Option<u64>) -> Result<()> {
		Err(unexpected())
	}
	async fn observe(&self) -> Result<()> {
		Ok(())
	}
}
struct Connector(Arc<State>);
#[async_trait]
impl ActivationConnector for Connector {
	async fn connect(&self) -> Result<Arc<dyn ActivationTransport>> {
		self.0.connecting.notify_one();
		std::future::pending().await
	}
}
fn driver(repository: &Repository) -> Arc<Runtime> {
	Runtime::new(
		Arc::new(repository.clone()),
		Arc::new(Connector(repository.0.clone())),
		Settings {
			recovery: Duration::from_millis(200),
			fallback: Duration::from_millis(100),
			test_pause_file: None,
			test_after_ack_pause_file: None,
		},
		true,
	)
}

#[rstest]
#[tokio::test]
async fn a_blocked_connector_cannot_hold_worker_slots_or_prevent_drain(repository: Repository) {
	// Arrange: connection establishment never completes; recovery has one durable step.
	let runtime = driver(&repository);
	let (stop, stopping) = watch::channel(false);
	let mut tasks = JoinSet::new();
	tasks.spawn(runtime.clone().run(stopping.clone()));
	tasks.spawn(runtime.worker(Arc::new(repository.clone()), stopping));
	// Act: shutdown arrives during the owned step, which must finish first.
	tokio::time::timeout(Duration::from_secs(1), repository.0.connecting.notified())
		.await
		.unwrap();
	tokio::time::timeout(Duration::from_secs(1), repository.0.entered.notified())
		.await
		.unwrap();
	stop.send_replace(true);
	tokio::task::yield_now().await;
	// Assert: visibility remains owned while the step drains.
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 1);
	assert_eq!(repository.0.recoveries.load(Ordering::Acquire), 1);
	repository.0.release.notify_one();
	tokio::time::timeout(Duration::from_secs(1), async {
		while let Some(result) = tasks.join_next().await {
			result.unwrap().unwrap();
		}
	})
	.await
	.unwrap();
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 0);
	assert_eq!(repository.0.recoveries.load(Ordering::Acquire), 1);
}

struct Pull(Arc<State>);
impl Drop for Pull {
	fn drop(&mut self) {
		self.0.pulls.fetch_sub(1, Ordering::AcqRel);
	}
}
struct Transport(Arc<State>);
#[async_trait]
impl ActivationTransport for Transport {
	fn disconnected(&self) -> bool {
		false
	}
	fn connected(&self) -> bool {
		true
	}
	async fn publish(&self, _: &Envelope, _: i64) -> Result<bool> {
		Err(unexpected())
	}
	async fn fetch(&self) -> Result<Option<Box<dyn ActivationDelivery>>> {
		let active = self.0.pulls.fetch_add(1, Ordering::AcqRel) + 1;
		self.0.peak_pulls.fetch_max(active, Ordering::AcqRel);
		let _pull = Pull(self.0.clone());
		std::future::pending().await
	}
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn finite_pulls_expire_before_the_same_slot_returns_to_recovery(repository: Repository) {
	// Arrange: a broker never supplies a message and no Run is ready.
	*repository.0.run.lock().unwrap() = None;
	let runtime = driver(&repository);
	runtime
		.broker
		.send_replace(Some(Arc::new(Transport(repository.0.clone()))));
	let (stop, stopping) = watch::channel(false);
	let mut tasks = JoinSet::new();
	tasks.spawn(runtime.worker(Arc::new(repository.clone()), stopping));
	// Act: cross the recovery deadline twice without speculative prefetch.
	for _ in 0..12 {
		tokio::task::yield_now().await;
		tokio::time::advance(Duration::from_millis(50)).await;
	}
	stop.send_replace(true);
	while let Some(result) = tasks.join_next().await {
		result.unwrap().unwrap();
	}
	// Assert: cancellation releases each pull before any following recovery claim.
	assert!(repository.0.recoveries.load(Ordering::Acquire) >= 2);
	assert_eq!(repository.0.peak_pulls.load(Ordering::Acquire), 1);
	assert_eq!(repository.0.pulls.load(Ordering::Acquire), 0);
	assert_eq!(repository.0.visibility.load(Ordering::Acquire), 0);
}

/// The instants at which a worker meets the lock, every `every` for `total`.
fn contended_every(start: Instant, every: Duration, total: Duration) -> Vec<Instant> {
	let mut at = vec![];
	let mut elapsed = Duration::ZERO;
	while elapsed <= total {
		at.push(start + elapsed);
		elapsed += every;
	}
	at
}

#[test]
fn contention_that_keeps_returning_warns_once_after_thirty_seconds() {
	// Arrange: a deferred delivery returns after 250 ms and the worker sleeps
	// 100 ms, so a contended pass comes about every 350 ms with idle passes
	// (which record nothing) between them.
	let start = Instant::now();
	let mut contention = Contention::default();

	// Act
	let warned = contended_every(start, Duration::from_millis(350), Duration::from_secs(60))
		.into_iter()
		.filter(|at| contention.contended(*at))
		.map(|at| at - start)
		.collect::<Vec<_>>();

	// Assert
	assert_eq!(warned.len(), 1, "{warned:?}");
	assert!(
		(Duration::from_secs(30)..Duration::from_secs(31)).contains(&warned[0]),
		"{warned:?}"
	);
}

#[test]
fn two_isolated_contentions_do_not_add_up_to_a_long_one() {
	// Arrange
	let start = Instant::now();
	let mut contention = Contention::default();

	// Act: one contention, then silence, then another 30 seconds later.
	let first = contention.contended(start);
	let second = contention.contended(start + Duration::from_secs(30));
	let still_second =
		contention.contended(start + Duration::from_secs(30) + Duration::from_millis(350));

	// Assert
	assert!(!first && !second && !still_second);
}

#[test]
fn a_new_contention_after_a_silence_warns_again() {
	// Arrange: the first contention warns.
	let start = Instant::now();
	let mut contention = Contention::default();
	let first = contended_every(start, Duration::from_secs(1), Duration::from_secs(31));
	assert_eq!(
		first
			.into_iter()
			.filter(|at| contention.contended(*at))
			.count(),
		1
	);

	// Act: a second one begins after a silence.
	let later = start + Duration::from_secs(120);
	let second = contended_every(later, Duration::from_secs(1), Duration::from_secs(31))
		.into_iter()
		.filter(|at| contention.contended(*at))
		.count();

	// Assert
	assert_eq!(second, 1);
}

#[test]
fn a_delivery_that_went_through_ends_the_contention() {
	// Arrange: 20 seconds of contention.
	let start = Instant::now();
	let mut contention = Contention::default();
	for at in contended_every(start, Duration::from_secs(1), Duration::from_secs(20)) {
		assert!(!contention.contended(at));
	}

	// Act: a claim succeeds, then the lock is contended again for 20 seconds.
	contention.clear();
	let resumed = start + Duration::from_secs(21);
	let warned = contended_every(resumed, Duration::from_secs(1), Duration::from_secs(20))
		.into_iter()
		.filter(|at| contention.contended(*at))
		.count();

	// Assert: neither stretch alone reached 30 seconds.
	assert_eq!(warned, 0);
}
