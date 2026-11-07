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
async fn an_accepted_connection_ends_the_failure_and_is_handed_back() {
	// Arrange
	let (_stop, mut stopping) = watch::channel(false);
	let mut failures = AcceptFailures::new("test.recovery");
	failures.settle(out_of_descriptors(), &mut stopping).await;
	assert!(failures.failing.is_some());
	// Act
	let accepted = failures.settle(Ok(7), &mut stopping).await;
	// Assert
	assert_eq!(accepted, Some(7));
	assert!(failures.failing.is_none());
}
