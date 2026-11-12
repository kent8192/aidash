use super::*;
use aidash_application::{
	Error,
	ports::capabilities::{
		read_recheck::{Outcome, Target},
		reconciliation::{OperationReconciliationRepository, OperationReconciliationScope},
	},
};
use aidash_domain::capabilities::operations::{processing::Receipt, reconciliation::Snapshot};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::Value;
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;
use uuid::Uuid;
#[derive(Default)]
struct Repository {
	ticks: AtomicUsize,
	entered: Notify,
	release: Notify,
	block: bool,
	fail: bool,
}
#[async_trait]
impl OperationReconciliationRepository for Repository {
	async fn snapshot(&self, _: Uuid) -> Result<Snapshot> {
		Err(Error::External("fixture has no operations".into()))
	}
	async fn begin(&self, _: &Snapshot) -> Result<Box<dyn OperationReconciliationScope + '_>> {
		Err(Error::Forbidden)
	}
	async fn withdraw(&self, _: &Snapshot) -> Result<()> {
		Err(Error::Forbidden)
	}
}
#[async_trait]
impl OperationProcessingRepository for Repository {
	async fn begin_receipt_output(
		&self,
		_: &Receipt,
	) -> Result<Box<dyn aidash_application::ports::capabilities::processing::ReceiptOutputScope + '_>>
	{
		Err(Error::Forbidden)
	}

	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		self.ticks.fetch_add(1, Ordering::SeqCst);
		self.entered.notify_one();
		if self.block {
			self.release.notified().await;
		}
		if self.fail {
			return Err(Error::External("Database error: Pool timed out".into()));
		}
		Ok(vec![])
	}
	async fn receipts(&self, _: Uuid) -> Result<Vec<Receipt>> {
		Ok(vec![])
	}
	async fn mark_acknowledged(&self, _: Uuid) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn storage_blocked(&self, _: Uuid, _: Value) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn runner_request(&self, _: &str, _: &str, _: Option<Value>) -> Result<Value> {
		Err(Error::Forbidden)
	}
}
#[rstest]
#[tokio::test]
async fn an_already_stopped_loop_does_not_admit_a_new_batch() {
	let repository = Repository::default();
	let (_stop, receiver) = watch::channel(true);
	execution_loop(&repository, receiver).await.unwrap();
	assert_eq!(repository.ticks.load(Ordering::SeqCst), 0);
}
#[rstest]
#[tokio::test]
async fn stop_waits_for_the_in_flight_batch_before_exiting() {
	let repository = Arc::new(Repository {
		block: true,
		..Default::default()
	});
	let (stop, receiver) = watch::channel(false);
	let running = repository.clone();
	let task = tokio::spawn(async move { execution_loop(&*running, receiver).await });
	repository.entered.notified().await;
	stop.send_replace(true);
	tokio::task::yield_now().await;
	assert!(!task.is_finished());
	repository.release.notify_one();
	task.await.unwrap().unwrap();
	assert_eq!(repository.ticks.load(Ordering::SeqCst), 1);
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn successful_batches_retain_the_three_hundred_millisecond_cadence() {
	let repository = Arc::new(Repository::default());
	let (stop, receiver) = watch::channel(false);
	let running = repository.clone();
	let task = tokio::spawn(async move { execution_loop(&*running, receiver).await });
	repository.entered.notified().await;
	tokio::task::yield_now().await;
	tokio::time::advance(Duration::from_millis(299)).await;
	tokio::task::yield_now().await;
	assert_eq!(repository.ticks.load(Ordering::SeqCst), 1);
	tokio::time::advance(Duration::from_millis(1)).await;
	repository.entered.notified().await;
	assert_eq!(repository.ticks.load(Ordering::SeqCst), 2);
	stop.send_replace(true);
	task.await.unwrap().unwrap();
}
/// #728: this returned the error, which stopped the whole process.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_sweep_that_fails_with_a_pool_timeout_does_not_end_the_loop() {
	// Arrange
	let repository = Arc::new(Repository {
		fail: true,
		..Default::default()
	});
	let (stop, receiver) = watch::channel(false);
	let running = repository.clone();
	let task = tokio::spawn(async move { execution_loop(&*running, receiver).await });
	// Act: each failed pass is followed by another.
	for _ in 0..4 {
		repository.entered.notified().await;
	}
	// Assert: it stops when asked, with no error.
	assert!(!task.is_finished());
	stop.send_replace(true);
	task.await.unwrap().unwrap();
	assert!(repository.ticks.load(Ordering::SeqCst) >= 4);
}
/// A slow check must not delay another Tenant's commands.
struct StuckRechecks;
#[async_trait]
impl ReadRecheckRepository for StuckRechecks {
	async fn targets(&self) -> Result<Vec<Target>> {
		std::future::pending().await
	}
	async fn check(&self, _: Target) -> Result<Outcome> {
		Err(Error::Forbidden)
	}
	async fn stop_refused(&self, _: Target) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn stop_unconfirmed(&self, _: Uuid) -> Result<()> {
		Err(Error::Forbidden)
	}
}

#[tokio::test(start_paused = true)]
async fn a_stuck_read_recheck_does_not_delay_the_operation_sweep() {
	// Arrange
	let repository = Repository::default();
	let (stop, receiver) = watch::channel(false);

	// Act
	let sweep = async {
		tokio::try_join!(
			Box::pin(execution_loop(&repository, receiver.clone())),
			async {
				tokio::select! {
					result = read_recheck_loop(&StuckRechecks, receiver.clone()) => result,
					() = async {
						while repository.ticks.load(Ordering::SeqCst) < 3 {
							tokio::time::sleep(Duration::from_millis(100)).await;
						}
						stop.send(true).unwrap();
					} => Ok(()),
				}
			},
		)
	};
	tokio::time::timeout(Duration::from_secs(60), sweep)
		.await
		.expect("the sweep keeps its own pace")
		.unwrap();

	// Assert
	assert!(repository.ticks.load(Ordering::SeqCst) >= 3);
}

#[async_trait]
impl ReadRecheckRepository for Repository {
	async fn targets(&self) -> Result<Vec<Target>> {
		Ok(vec![])
	}
	async fn check(&self, _: Target) -> Result<Outcome> {
		Err(Error::Forbidden)
	}
	async fn stop_refused(&self, _: Target) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn stop_unconfirmed(&self, _: Uuid) -> Result<()> {
		Err(Error::Forbidden)
	}
}

#[async_trait]
impl aidash_application::ports::capabilities::pod_usage::PodUsageRepository for Repository {
	async fn pending_allocations(
		&self,
		_: Option<Uuid>,
	) -> Result<aidash_application::ports::capabilities::pod_usage::AllocationPage> {
		Ok(
			aidash_application::ports::capabilities::pod_usage::AllocationPage {
				instance: Uuid::nil(),
				receipts: vec![],
				next_after: None,
				pending_evidence: 0,
			},
		)
	}
	async fn deliver_allocation(&self, _: aidash_application::usage::PodTime) -> Result<()> {
		Err(Error::Forbidden)
	}
	async fn acknowledge_allocation(
		&self,
		_: &aidash_application::ports::capabilities::pod_usage::AllocationReceipt,
	) -> Result<()> {
		Err(Error::Forbidden)
	}
}
