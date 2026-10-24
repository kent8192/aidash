use super::*;
use aidash_application::{
	Error,
	ports::capabilities::reconciliation::{
		OperationReconciliationRepository, OperationReconciliationScope,
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
use tokio::sync::{Barrier, Notify};
use uuid::Uuid;
#[derive(Default)]
struct Repository {
	ticks: AtomicUsize,
	entered: Notify,
	release: Notify,
	block: bool,
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
	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		self.ticks.fetch_add(1, Ordering::SeqCst);
		self.entered.notify_one();
		if self.block {
			self.release.notified().await;
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
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::SeqCst);
	}
}
struct Jobs {
	barrier: Barrier,
	active: Arc<AtomicUsize>,
	started: AtomicUsize,
}
impl Jobs {
	async fn job(&self, failure: bool) -> Result<()> {
		self.active.fetch_add(1, Ordering::SeqCst);
		self.started.fetch_add(1, Ordering::SeqCst);
		let _guard = Guard(self.active.clone());
		self.barrier.wait().await;
		if failure {
			Err(Error::External("network failed".into()))
		} else {
			std::future::pending().await
		}
	}
}
#[async_trait]
impl CapabilityBackgroundJobs for Jobs {
	async fn network(&self, _: watch::Receiver<bool>) -> Result<()> {
		self.job(true).await
	}
	async fn references(&self, _: watch::Receiver<bool>) -> Result<()> {
		self.job(false).await
	}
	async fn cleanup(&self, _: watch::Receiver<bool>) -> Result<()> {
		self.job(false).await
	}
	async fn reclamation(&self, _: watch::Receiver<bool>) -> Result<()> {
		self.job(false).await
	}
}
#[rstest]
#[tokio::test]
async fn a_fatal_background_job_drops_all_other_owned_job_futures() {
	let repository = Repository {
		block: true,
		..Default::default()
	};
	let active = Arc::new(AtomicUsize::new(0));
	let jobs = Jobs {
		barrier: Barrier::new(4),
		active: active.clone(),
		started: AtomicUsize::new(0),
	};
	let (_stop, receiver) = watch::channel(false);
	assert!(
		matches!(run(&repository,&jobs,receiver).await,Err(Error::External(message)) if message=="network failed")
	);
	assert_eq!(jobs.started.load(Ordering::SeqCst), 4);
	assert_eq!(active.load(Ordering::SeqCst), 0);
}
