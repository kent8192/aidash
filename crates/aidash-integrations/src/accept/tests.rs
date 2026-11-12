use super::*;
use rstest::rstest;

fn out_of_descriptors() -> io::Result<()> {
	Err(io::Error::from_raw_os_error(libc::EMFILE))
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_failed_accept_waits_before_the_next_try() {
	// Arrange
	let (_stop, mut stopping) = watch::channel(false);
	let mut failures = AcceptFailures::new("test.wait");
	let started = Instant::now();
	// Act
	let accepted = failures.settle(out_of_descriptors(), &mut stopping).await;
	// Assert
	assert_eq!(accepted, None);
	assert_eq!(started.elapsed(), RETRY_AFTER);
}

#[rstest]
#[case::asked_to_stop(false)]
#[case::stop_signal_gone(true)]
#[tokio::test(start_paused = true)]
async fn stopping_ends_the_wait(#[case] signal_gone: bool) {
	// Arrange
	let (stop, mut stopping) = watch::channel(false);
	let mut failures = AcceptFailures::new("test.stop");
	if signal_gone {
		drop(stop);
	} else {
		stop.send_replace(true);
	}
	let started = Instant::now();
	// Act
	failures.settle(out_of_descriptors(), &mut stopping).await;
	// Assert
	assert_eq!(started.elapsed(), Duration::ZERO);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_lasting_failure_logs_once_a_minute() {
	// Arrange
	let mut failures = AcceptFailures::new("test.log");
	let error = out_of_descriptors().unwrap_err();
	let started = Instant::now();
	failures.record_failure(&error);
	// Act and assert: a repeat inside the minute logs nothing.
	tokio::time::advance(FAILURE_LOG_EVERY - RETRY_AFTER).await;
	failures.record_failure(&error);
	assert_eq!(failures.failing.as_ref().unwrap().logged, started);
	// The first repeat past the minute logs and starts the next minute.
	tokio::time::advance(RETRY_AFTER).await;
	failures.record_failure(&error);
	let failing = failures.failing.as_ref().unwrap();
	assert_eq!(failing.logged, started + FAILURE_LOG_EVERY);
	assert_eq!(failing.count, 3);
	assert_eq!(failing.since, started);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn one_accepted_connection_does_not_end_a_failure() {
	// Arrange: a descriptor comes free in the middle of a shortage.
	let (_stop, mut stopping) = watch::channel(false);
	let mut failures = AcceptFailures::new("test.flapping");
	let started = Instant::now();
	failures.settle(out_of_descriptors(), &mut stopping).await;
	// Act
	let accepted = failures.settle(Ok(7), &mut stopping).await;
	failures.settle(out_of_descriptors(), &mut stopping).await;
	// Assert: the connection is handed back and the shortage carries on.
	assert_eq!(accepted, Some(7));
	let failing = failures.failing.as_ref().unwrap();
	assert_eq!(failing.since, started);
	assert_eq!(failing.count, 2);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_minute_without_a_failure_ends_it() {
	// Arrange
	let (_stop, mut stopping) = watch::channel(false);
	let mut failures = AcceptFailures::new("test.recovery");
	failures.settle(out_of_descriptors(), &mut stopping).await;
	// Act and assert: still failing just short of the minute.
	tokio::time::advance(RECOVERED_AFTER - RETRY_AFTER - RETRY_AFTER).await;
	failures.settle(Ok(7), &mut stopping).await;
	assert!(failures.failing.is_some());
	tokio::time::advance(RETRY_AFTER + RETRY_AFTER).await;
	failures.settle(Ok(7), &mut stopping).await;
	assert!(failures.failing.is_none());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_failure_after_a_quiet_minute_starts_a_new_one() {
	// Arrange
	let (_stop, mut stopping) = watch::channel(false);
	let mut failures = AcceptFailures::new("test.new");
	failures.settle(out_of_descriptors(), &mut stopping).await;
	tokio::time::advance(RECOVERED_AFTER).await;
	let started = Instant::now();
	// Act
	failures.settle(out_of_descriptors(), &mut stopping).await;
	// Assert
	let failing = failures.failing.as_ref().unwrap();
	assert_eq!(failing.since, started);
	assert_eq!(failing.count, 1);
}
