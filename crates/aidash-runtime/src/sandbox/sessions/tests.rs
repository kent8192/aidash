use super::*;
use rstest::rstest;
use std::{
	sync::atomic::{AtomicBool, AtomicUsize, Ordering},
	time::Duration,
};
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
	fn drop(&mut self) {
		self.0.fetch_add(1, Ordering::SeqCst);
	}
}
fn id() -> SessionId {
	SessionId::from_u128(9)
}
#[rstest]
#[tokio::test]
async fn closure_rejects_new_admission_and_drains_an_already_running_job() {
	// Arrange an owned job that remains active until its external wait is released.
	let sessions = Sessions::default();
	let entered = Arc::new(Notify::new());
	let release = Arc::new(Notify::new());
	let dropped = Arc::new(AtomicUsize::new(0));
	let guard = Guard(dropped.clone());
	let started = entered.clone();
	let waiting = release.clone();
	sessions
		.permit()
		.unwrap()
		.spawn(id(), async move {
			let _guard = guard;
			started.notify_one();
			waiting.notified().await;
			Ok(())
		})
		.unwrap();
	entered.notified().await;
	let (stop, receiver) = watch::channel(false);
	let run = sessions.clone().run(receiver);
	tokio::pin!(run);
	assert!(futures_util::poll!(&mut run).is_pending());
	// Act by closing admission before the stop notification reaches the driver.
	sessions.close();
	assert!(matches!(sessions.permit(), Err(Error::Conflict(_))));
	stop.send_replace(true);
	assert!(futures_util::poll!(&mut run).is_pending());
	assert_eq!(dropped.load(Ordering::SeqCst), 0);
	// Assert the accepted job can finish normally and the group cannot reopen admission.
	release.notify_one();
	run.await.unwrap();
	assert_eq!(dropped.load(Ordering::SeqCst), 1);
	assert!(sessions.permit().is_err());
	assert!(sessions.drained());
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn a_pending_admission_remains_in_the_drain_set_until_launch_or_rollback(
	#[case] launch: bool,
) {
	let sessions = Sessions::default();
	let permit = sessions.permit().unwrap();
	let (_stop, receiver) = watch::channel(true);
	let run = sessions.clone().run(receiver);
	tokio::pin!(run);
	assert!(futures_util::poll!(&mut run).is_pending());
	assert!(sessions.permit().is_err());
	let completed = Arc::new(AtomicBool::new(false));
	if launch {
		let completed = completed.clone();
		permit
			.spawn(id(), async move {
				completed.store(true, Ordering::SeqCst);
				Ok(())
			})
			.unwrap();
	} else {
		drop(permit);
	}
	run.await.unwrap();
	assert_eq!(completed.load(Ordering::SeqCst), launch);
	assert!(sessions.drained());
}
#[rstest]
#[tokio::test]
async fn dropping_the_driver_aborts_jobs_even_when_a_server_handle_remains() {
	let sessions = Sessions::default();
	let dropped = Arc::new(AtomicUsize::new(0));
	let guard = Guard(dropped.clone());
	sessions
		.permit()
		.unwrap()
		.spawn(id(), async move {
			let _guard = guard;
			std::future::pending::<Result<()>>().await
		})
		.unwrap();
	let (_stop, receiver) = watch::channel(false);
	{
		let run = sessions.clone().run(receiver);
		tokio::pin!(run);
		assert!(futures_util::poll!(&mut run).is_pending());
	}
	tokio::task::yield_now().await;
	assert_eq!(dropped.load(Ordering::SeqCst), 1);
	assert!(sessions.permit().is_err());
	assert!(sessions.inner.state.lock().unwrap().cancelled);
}
#[rstest]
#[tokio::test]
async fn dropping_the_last_owner_cancels_jobs_without_a_detached_driver() {
	let sessions = Sessions::default();
	let dropped = Arc::new(AtomicUsize::new(0));
	let guard = Guard(dropped.clone());
	sessions
		.permit()
		.unwrap()
		.spawn(id(), async move {
			let _guard = guard;
			std::future::pending::<Result<()>>().await
		})
		.unwrap();
	drop(sessions);
	tokio::task::yield_now().await;
	assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
#[rstest]
#[tokio::test]
async fn cancellation_after_drain_deadline_rejects_launch_and_drops_the_unstarted_job() {
	let sessions = Sessions::default();
	let permit = sessions.permit().unwrap();
	let (_stop, receiver) = watch::channel(false);
	{
		let run = sessions.clone().run(receiver);
		tokio::pin!(run);
		assert!(futures_util::poll!(&mut run).is_pending());
	}
	let attempted = Arc::new(AtomicBool::new(false));
	let dropped = Arc::new(AtomicUsize::new(0));
	let guard = Guard(dropped.clone());
	let called = attempted.clone();
	let error = permit
		.spawn(id(), async move {
			let _guard = guard;
			called.store(true, Ordering::SeqCst);
			Ok(())
		})
		.unwrap_err();
	assert_eq!(
		error.to_string(),
		"sandbox runtime stopped before execution"
	);
	assert!(!attempted.load(Ordering::SeqCst));
	assert_eq!(dropped.load(Ordering::SeqCst), 1);
	assert_eq!(sessions.inner.state.lock().unwrap().pending, 0);
}
#[rstest]
#[tokio::test]
async fn failed_jobs_do_not_stop_the_remaining_session_or_close_admission() {
	let sessions = Sessions::default();
	let entered = Arc::new(Notify::new());
	let release = Arc::new(Notify::new());
	let dropped = Arc::new(AtomicUsize::new(0));
	let started = entered.clone();
	let waiting = release.clone();
	let guard = Guard(dropped.clone());
	sessions
		.permit()
		.unwrap()
		.spawn(SessionId::from_u128(8), async {
			Err(Error::External("session failed".into()))
		})
		.unwrap();
	sessions
		.permit()
		.unwrap()
		.spawn(id(), async move {
			let _guard = guard;
			started.notify_one();
			waiting.notified().await;
			Ok(())
		})
		.unwrap();
	entered.notified().await;
	let (stop, receiver) = watch::channel(false);
	let run = sessions.clone().run(receiver);
	tokio::pin!(run);
	assert!(futures_util::poll!(&mut run).is_pending());
	drop(sessions.permit().unwrap());
	assert_eq!(dropped.load(Ordering::SeqCst), 0);
	stop.send_replace(true);
	release.notify_one();
	run.await.unwrap();
	assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
#[rstest]
#[tokio::test]
async fn a_duplicate_driver_does_not_cancel_the_owner() {
	let sessions = Sessions::default();
	let (stop, receiver) = watch::channel(false);
	let owner = sessions.clone().run(receiver);
	tokio::pin!(owner);
	assert!(futures_util::poll!(&mut owner).is_pending());
	let error = sessions.clone().run(stop.subscribe()).await.unwrap_err();
	assert_eq!(error.to_string(), "sandbox supervisor is already running");
	drop(sessions.permit().unwrap());
	stop.send_replace(true);
	owner.await.unwrap();
}
#[rstest]
#[tokio::test(start_paused = true)]
async fn the_process_supervisor_applies_its_shared_drain_deadline_to_sandbox_jobs() {
	let sessions = Sessions::default();
	let dropped = Arc::new(AtomicUsize::new(0));
	let guard = Guard(dropped.clone());
	sessions
		.permit()
		.unwrap()
		.spawn(id(), async move {
			let _guard = guard;
			std::future::pending::<Result<()>>().await
		})
		.unwrap();
	let mut supervisor = crate::Supervisor::new(Duration::from_secs(20));
	supervisor.spawn_worker(sessions.clone().run(supervisor.stop_receiver()));
	let error = supervisor.shutdown().await.unwrap_err();
	assert_eq!(error.to_string(), "worker drain deadline reached");
	tokio::task::yield_now().await;
	assert_eq!(dropped.load(Ordering::SeqCst), 1);
	assert!(sessions.permit().is_err());
}
