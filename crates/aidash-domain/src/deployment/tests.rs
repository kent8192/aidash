use super::*;
use rstest::rstest;

fn pod(namespace: &str, release: &str, uid: &str) -> ScopedObservation<Pod> {
	ScopedObservation {
		namespace: namespace.into(),
		application: "aidash".into(),
		release: release.into(),
		uid: Some(uid.into()),
		value: Pod {
			name: uid.into(),
			role: "worker".into(),
			phase: "Pending".into(),
			ready: false,
			terminating: false,
			restarts: 0,
			conditions: vec![],
		},
	}
}

fn event(namespace: &str, uid: Option<&str>) -> EventObservation {
	EventObservation {
		namespace: namespace.into(),
		object_uid: uid.map(str::to_owned),
		value: DeploymentEvent {
			object: "pod".into(),
			kind: "Warning".into(),
			reason: "BackOff".into(),
			message: "restart pending".into(),
			count: 1,
			time: None,
		},
	}
}

#[rstest]
fn observations_and_events_stay_within_the_configured_release() {
	// Arrange
	let inventory = DeploymentInventory {
		pods: vec![
			pod("tenant-a", "release-a", "own"),
			pod("tenant-a", "release-b", "other-release"),
			pod("tenant-b", "release-a", "other-namespace"),
		],
		events: vec![
			event("tenant-a", Some("own")),
			event("tenant-a", Some("other-release")),
			event("tenant-b", Some("own")),
			event("tenant-a", None),
		],
		..Default::default()
	};
	// Act
	let status = DeploymentStatus::snapshot("tenant-a", "release-a", inventory);
	// Assert
	assert_eq!(status.pods.len(), 1);
	assert_eq!(status.pods[0].name, "own");
	assert_eq!(status.events.len(), 1);
	assert_eq!(status.namespace.as_deref(), Some("tenant-a"));
	assert_eq!(status.release.as_deref(), Some("release-a"));
}

#[rstest]
#[case("release-a", true)]
#[case("release,other=value", false)]
#[case("-release", false)]
#[case("", false)]
#[case("Release", false)]
fn label_validation_preserves_selector_boundaries(#[case] label: &str, #[case] valid: bool) {
	assert_eq!(valid_label(label), valid);
}
