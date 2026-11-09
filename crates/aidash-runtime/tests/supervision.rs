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
	supervisor.spawn_worker("test worker", async move {
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
	supervisor.spawn_worker("test worker", async move {
		token.stopped().await;
		completed.store(true, Ordering::SeqCst);
		Ok(())
	});
	supervisor.spawn_service("test service", async {
		Err(Error::External("broker unavailable".into()))
	});

	let result = supervisor.run(pending()).await;

	let message = result.unwrap_err().to_string();
	assert!(message.contains("broker unavailable"), "{message}");
	assert!(message.contains("test service"), "{message}");
	assert!(drained.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_panicking_service_is_named_when_it_stops_the_process() {
	let mut supervisor = Supervisor::new(Duration::from_secs(20));
	supervisor.spawn_service("capabilities.network", async { panic!("boom") });

	let message = supervisor.run(pending()).await.unwrap_err().to_string();

	assert!(message.contains("capabilities.network"), "{message}");
}

#[tokio::test]
async fn requested_shutdown_wins_over_a_service_observing_the_same_signal() {
	for _ in 0..32 {
		let mut supervisor = Supervisor::new(Duration::from_secs(20));
		let mut token = supervisor.stop_token();
		let drained = Arc::new(AtomicBool::new(false));
		let completed = drained.clone();
		supervisor.spawn_worker("test worker", async move {
			token.stopped().await;
			completed.store(true, Ordering::SeqCst);
			Ok(())
		});
		let (exited, observed) = oneshot::channel();
		supervisor.spawn_service("test service", async move {
			exited.send(()).unwrap();
			Ok(())
		});
		observed.await.unwrap();
		tokio::task::yield_now().await;
		// Both the coordinator's signal and its exiting metrics task are ready.
		supervisor.run(async {}).await.unwrap();
		assert!(drained.load(Ordering::SeqCst));
	}
}

#[rstest::rstest]
#[tokio::test]
async fn requested_shutdown_takes_precedence_over_a_completed_service() {
	let drained = Arc::new(AtomicBool::new(false));
	let completed = drained.clone();
	let mut supervisor = Supervisor::new(Duration::from_secs(20));
	let mut token = supervisor.stop_token();
	supervisor.spawn_worker("test worker", async move {
		token.stopped().await;
		completed.store(true, Ordering::SeqCst);
		Ok(())
	});
	let (finished, ready) = oneshot::channel();
	supervisor.spawn_service("test service", async move {
		finished.send(()).unwrap();
		Ok(())
	});
	ready.await.unwrap();

	let admission_closed = Arc::new(AtomicBool::new(false));
	let closed = admission_closed.clone();
	let result = supervisor
		.run_with_shutdown(async {}, move || {
			closed.store(true, Ordering::SeqCst);
		})
		.await;

	assert!(result.is_ok(), "{result:?}");
	assert!(admission_closed.load(Ordering::SeqCst));
	assert!(drained.load(Ordering::SeqCst));
}

#[rstest::rstest]
#[tokio::test(start_paused = true)]
async fn drain_timeout_aborts_uncooperative_workers() {
	let released = Arc::new(AtomicBool::new(false));
	let mut supervisor = Supervisor::new(Duration::from_secs(20));
	let resource = Released(released.clone());
	supervisor.spawn_worker("test worker", async move {
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
