use super::*;
use aidash_application::{
	Error,
	ports::capabilities::outbound::{FetchPolicy, OutboundScope},
};
use aidash_domain::capabilities::records::Record;
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

struct Repository {
	scans: AtomicUsize,
	entered: Notify,
	permits: Semaphore,
	active: AtomicUsize,
	maximum: AtomicUsize,
	snapshots: AtomicUsize,
	ids: Vec<Uuid>,
	blocked: bool,
}
impl Repository {
	fn new(count: u128, blocked: bool) -> Self {
		Self {
			scans: AtomicUsize::new(0),
			entered: Notify::new(),
			permits: Semaphore::new(0),
			active: AtomicUsize::new(0),
			maximum: AtomicUsize::new(0),
			snapshots: AtomicUsize::new(0),
			ids: (1..=count).map(Uuid::from_u128).collect(),
			blocked,
		}
	}
}
struct Guard<'a>(&'a AtomicUsize);
impl Drop for Guard<'_> {
	fn drop(&mut self) {
		self.0.fetch_sub(1, Ordering::SeqCst);
	}
}
#[async_trait]
impl OutboundRepository for Repository {
	fn fetch_policy(&self) -> FetchPolicy {
		FetchPolicy {
			origins: vec![],
			output_bytes: 0,
		}
	}
	async fn active_operations(&self) -> Result<Vec<Uuid>> {
		self.scans.fetch_add(1, Ordering::SeqCst);
		self.entered.notify_one();
		Ok(self.ids.clone())
	}
	async fn snapshot(&self, id: Uuid) -> Result<Record> {
		self.snapshots.fetch_add(1, Ordering::SeqCst);
		let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
		self.maximum.fetch_max(active, Ordering::SeqCst);
		let _guard = Guard(&self.active);
		self.entered.notify_one();
		if self.blocked {
			let _permit = self.permits.acquire().await.unwrap();
		}
		Ok(Record {
			id,
			tenant: "tenant".into(),
			owner: "owner".into(),
			area_id: None,
			kind: "outbound".into(),
			state: "attempted".into(),
			revision: 1,
			data: json!({}),
			expires_at: None,
		})
	}
	async fn begin(&self, _: &Record) -> Result<Box<dyn OutboundScope + '_>> {
		Err(Error::Forbidden)
	}
	async fn fail(&self, _: Uuid, message: &str) -> Result<()> {
		assert_eq!(message, "OUTBOUND_EFFECT_UNCERTAIN");
		Ok(())
	}
}
#[async_trait]
impl OutboundTransport for Repository {
	async fn fetch(&self, _: &str, _: &Value, _: &FetchPolicy) -> Result<(u16, Vec<u8>, String)> {
		Err(Error::Forbidden)
	}
}

#[rstest]
#[tokio::test]
async fn stopping_before_admission_does_not_scan_outbound_work() {
	let repository = Repository::new(1, false);
	let (_stop, receiver) = watch::channel(true);
	run(&repository, &repository, receiver).await.unwrap();
	assert_eq!(repository.scans.load(Ordering::SeqCst), 0);
}

#[rstest]
#[tokio::test]
async fn outbound_concurrency_is_bounded_and_stop_drains_the_owned_batch() {
	let repository = Repository::new(16, true);
	let (stop, receiver) = watch::channel(false);
	let finished = AtomicBool::new(false);
	let work = async {
		let result = run(&repository, &repository, receiver).await;
		finished.store(true, Ordering::SeqCst);
		result
	};
	let observe = async {
		while repository.active.load(Ordering::SeqCst) < 8 {
			repository.entered.notified().await;
		}
		stop.send_replace(true);
		assert!(!finished.load(Ordering::SeqCst));
		assert_eq!(repository.maximum.load(Ordering::SeqCst), 8);
		assert_eq!(repository.scans.load(Ordering::SeqCst), 1);
		repository.permits.add_permits(16);
	};
	let (result, ()) = tokio::join!(work, observe);
	result.unwrap();
	assert_eq!(repository.scans.load(Ordering::SeqCst), 1);
	assert_eq!(repository.snapshots.load(Ordering::SeqCst), 16);
	assert_eq!(repository.maximum.load(Ordering::SeqCst), 8);
	assert_eq!(repository.active.load(Ordering::SeqCst), 0);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn an_empty_batch_retains_the_original_polling_interval() {
	let repository = Repository::new(0, false);
	let (stop, receiver) = watch::channel(false);
	let observe = async {
		repository.entered.notified().await;
		tokio::time::advance(Duration::from_millis(299)).await;
		tokio::task::yield_now().await;
		assert_eq!(repository.scans.load(Ordering::SeqCst), 1);
		tokio::time::advance(Duration::from_millis(1)).await;
		while repository.scans.load(Ordering::SeqCst) < 2 {
			repository.entered.notified().await;
		}
		assert_eq!(repository.scans.load(Ordering::SeqCst), 2);
		stop.send_replace(true);
	};
	let (result, ()) = tokio::join!(run(&repository, &repository, receiver), observe);
	result.unwrap();
}
