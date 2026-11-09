use super::*;
use aidash_application::Error;
use rstest::rstest;
use std::{
	collections::VecDeque,
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
};

fn pool_timeout() -> Error {
	Error::External("Database error: Pool timed out".into())
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_failed_pass_waits_longer_each_time_up_to_the_cap() {
	// Arrange
	let mut passes = Passes::new("test.backoff", Duration::from_millis(300));
	// Act and assert
	let mut delays = Vec::new();
	for _ in 0..6 {
		passes.run(async { Err::<(), _>(pool_timeout()) }).await;
		delays.push(passes.delay());
	}
	assert!(delays[0] >= Duration::from_millis(600), "{delays:?}");
	assert!(delays[1] >= Duration::from_millis(1200), "{delays:?}");
	assert!(delays[2] >= Duration::from_millis(2400), "{delays:?}");
	assert!(delays.iter().all(|delay| *delay <= MAX_DELAY), "{delays:?}");
	assert!(delays[5] >= Duration::from_millis(4900) || delays[5] == MAX_DELAY);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_successful_pass_restores_the_normal_interval() {
	// Arrange
	let mut passes = Passes::new("test.recovery", Duration::from_millis(300));
	passes.run(async { Err::<(), _>(pool_timeout()) }).await;
	assert!(passes.delay() > Duration::from_millis(300));
	// Act
	passes.run(async { Ok(()) }).await;
	// Assert
	assert_eq!(passes.delay(), Duration::from_millis(300));
	assert!(passes.failing.is_none());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_slow_pass_is_left_to_finish() {
	// Arrange
	let mut passes = Passes::new("test.slow", Duration::from_millis(300));
	let finished = Arc::new(AtomicUsize::new(0));
	let done = finished.clone();
	// Act: longer than every reporting period.
	passes
		.run(async move {
			tokio::time::sleep(SLOW_PASS_LOG_EVERY * 3).await;
			done.fetch_add(1, Ordering::SeqCst);
			Ok(())
		})
		.await;
	// Assert
	assert_eq!(finished.load(Ordering::SeqCst), 1);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn pause_ends_as_soon_as_the_loop_is_asked_to_stop() {
	// Arrange: the loop is in its longest wait.
	let mut passes = Passes::new("test.stop", Duration::from_millis(300));
	for _ in 0..6 {
		passes.run(async { Err::<(), _>(pool_timeout()) }).await;
	}
	let (stop, mut stopping) = watch::channel(false);
	let started = Instant::now();
	// Act
	stop.send_replace(true);
	passes.pause(&mut stopping).await;
	// Assert
	assert_eq!(started.elapsed(), Duration::ZERO);
}

/// A loop keeps going through a failure and stops only when asked.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_loop_survives_failures_until_it_is_stopped() {
	// Arrange
	let outcomes = Arc::new(Mutex::new(VecDeque::from([
		Err(pool_timeout()),
		Err(pool_timeout()),
		Ok(()),
	])));
	let (stop, mut stopping) = watch::channel(false);
	let mut passes = Passes::new("test.loop", Duration::from_millis(300));
	let mut count = 0;
	// Act
	while !*stopping.borrow() {
		let outcome = outcomes.lock().unwrap().pop_front();
		count += 1;
		passes
			.run(async move {
				match outcome {
					Some(outcome) => outcome,
					None => Ok(()),
				}
			})
			.await;
		if count == 4 {
			stop.send_replace(true);
		}
		passes.pause(&mut stopping).await;
	}
	// Assert
	assert_eq!(count, 4);
	assert!(passes.failing.is_none());
}
