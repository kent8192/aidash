use super::run;
use metrics_exporter_prometheus::PrometheusBuilder;
use rstest::rstest;
use std::{
	sync::{Arc, Mutex},
	time::Duration,
};

const SERVICE: &str = "test.watched";

fn pass_age(rendered: &str) -> f64 {
	rendered
		.lines()
		.find(|line| {
			line.starts_with("aidash_background_service_pass_age_seconds{")
				&& line.contains(&format!("service=\"{SERVICE}\""))
		})
		.and_then(|line| line.rsplit(' ').next())
		.and_then(|value| value.parse().ok())
		.expect("the loop reports its pass age")
}

/// A purge stuck on a vanished server must show, not hold retention silent.
#[rstest]
#[tokio::test(start_paused = true)]
async fn a_stalled_tick_shows_in_pass_age() {
	// Arrange: a thread-local recorder avoids modifying process-global telemetry.
	let recorder = PrometheusBuilder::new().build_recorder();
	let handle = recorder.handle();
	let _recorder = metrics::set_default_local_recorder(&recorder);
	// Act: the first tick never returns.
	let stalled = tokio::time::timeout(
		Duration::from_secs(95),
		run(SERVICE, Duration::from_secs(60), |_| {
			std::future::pending::<()>()
		}),
	)
	.await;
	// Assert: the loop is still waiting on it, and says for how long.
	assert!(stalled.is_err());
	assert!(pass_age(&handle.render()) >= 90.0);
}

/// Ticks keep their count and their cadence, which decide when the hourly purges run.
#[rstest]
#[tokio::test(start_paused = true)]
async fn ticks_are_numbered_and_run_once_per_interval() {
	// Arrange
	let seen = Arc::new(Mutex::new(Vec::new()));
	let recorded = seen.clone();
	// Act
	let _ = tokio::time::timeout(
		Duration::from_secs(125),
		run(SERVICE, Duration::from_secs(60), move |ticks| {
			let recorded = recorded.clone();
			async move { recorded.lock().unwrap().push(ticks) }
		}),
	)
	.await;
	// Assert
	assert_eq!(*seen.lock().unwrap(), vec![0, 1, 2]);
}
