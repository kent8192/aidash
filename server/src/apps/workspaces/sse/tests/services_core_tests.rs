//! Unit tests for services::core.
use super::*;
#[rstest::rstest]
#[tokio::test]
async fn wakeup_survives_read_to_wait_gap_and_scopes_are_reclaimed(service: Service) {
	let workspace = Uuid::new_v4();
	let mut selected = service.register(Some(workspace));
	let mut all = service.register(None);
	let other = service.register(Some(Uuid::new_v4()));
	selected.take_changes();
	service.notify(Some(workspace), false);
	service.notify(Some(workspace), false);
	tokio::time::timeout(Duration::from_millis(100), selected.receiver.changed())
		.await
		.unwrap()
		.unwrap();
	assert_eq!(selected.take_changes().0, Reasons::NOTIFICATION.0);
	assert_eq!(all.take_changes().0, Reasons::NOTIFICATION.0);
	assert!(!other.receiver.has_changed().unwrap());
	service.notify(None, true);
	service.notify(Some(workspace), false);
	assert_eq!(
		selected.take_changes().0,
		Reasons::RECONNECT.0 | Reasons::NOTIFICATION.0
	);
	drop((selected, all, other));
	assert_eq!(service.snapshot().registered_scopes, 0);
}
#[test]
fn settings_reject_disabled_or_unbounded_recovery() {
	for bad in ["0", "249", "60001", "invalid"] {
		assert!(
			Settings::from_values(
				|key| (key == "AIDASH_SSE_RECONCILE_INTERVAL_MS").then(|| bad.into())
			)
			.is_err()
		);
	}
	for bad in ["0", "301", "invalid", "-1"] {
		assert!(
			Settings::from_values(
				|key| (key == "AIDASH_SSE_BACKPRESSURE_TIMEOUT_SECONDS").then(|| bad.into())
			)
			.is_err()
		);
	}
	assert_eq!(
		Settings::from_values(|_| None).unwrap().reconcile_interval,
		Duration::from_secs(5)
	);
}

#[rstest::fixture]
fn service() -> Service {
	Service::new(Settings::default())
}
