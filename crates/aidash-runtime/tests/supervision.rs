use aidash_application::Error;
use aidash_runtime::Supervisor;
use std::{
	future::pending,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};
use tokio::sync::oneshot;

struct Released(Arc<AtomicBool>);

impl Drop for Released {
	fn drop(&mut self) {
		self.0.store(true, Ordering::SeqCst);
	}
}

#[rstest::rstest]
#[tokio::test]
async fn supervisor_drop_cancels_owned_tasks() {
	// Arrange: wait until the worker has acquired its owned resource.
	let released = Arc::new(AtomicBool::new(false));
	let mut supervisor = Supervisor::new(Duration::from_secs(20));
	let (started, ready) = oneshot::channel();
	let resource = Released(released.clone());
	supervisor.spawn_worker(async move {
		let _resource = resource;
		started.send(()).unwrap();
		pending::<()>().await;
		Ok(())
	});
	ready.await.unwrap();

	// Act: early-return cancellation must not detach work.
	drop(supervisor);
	tokio::task::yield_now().await;

	// Assert: cancellation dropped the worker's resource.
	assert!(released.load(Ordering::SeqCst));
}

#[rstest::rstest]
#[tokio::test]
async fn service_failure_stops_and_drains_workers() {
	let drained = Arc::new(AtomicBool::new(false));
	let completed = drained.clone();
	let mut supervisor = Supervisor::new(Duration::from_secs(20));
	let mut token = supervisor.stop_token();
	supervisor.spawn_worker(async move {
		token.stopped().await;
		completed.store(true, Ordering::SeqCst);
		Ok(())
	});
	supervisor.spawn_service(async { Err(Error::External("broker unavailable".into())) });

	let result = supervisor.run(pending()).await;

	assert!(
		result
			.unwrap_err()
			.to_string()
			.contains("broker unavailable")
	);
	assert!(drained.load(Ordering::SeqCst));
}

#[rstest::rstest]
#[tokio::test(start_paused = true)]
async fn drain_timeout_aborts_uncooperative_workers() {
	let released = Arc::new(AtomicBool::new(false));
	let mut supervisor = Supervisor::new(Duration::from_secs(20));
	let resource = Released(released.clone());
	supervisor.spawn_worker(async move {
		let _resource = resource;
		pending::<()>().await;
		Ok(())
	});

	let result = supervisor.shutdown().await;

	assert_eq!(
		result.unwrap_err().to_string(),
		"worker drain deadline reached"
	);
	assert!(released.load(Ordering::SeqCst));
}
