//! Rolling deployment and rollback: a Run whose persisted state needs a capability
//! is only claimed by workers that advertise it, but any worker finishes its cancellation.
use super::*;
use aidash_domain::run_state::encode;

const OLD_WORKER: &[PersistedCapability] = &[];
const NEW_WORKER: &[PersistedCapability] = &[PersistedCapability::ToolBatchState];

/// The envelope a new worker writes once its state needs `tool_batch_state`.
fn reserved(now: DateTime<Utc>) -> RawRun {
	let mut row = raw(now);
	row.pending["requires"] = serde_json::json!(["tool_batch_state"]);
	row
}
fn deploy(repository: &Repository, capabilities: &'static [PersistedCapability]) {
	*repository.0.capabilities.lock().unwrap() = capabilities;
}
async fn lease_as(repository: &Repository) -> Option<Run> {
	let mut scope = Scope {
		state: repository.0.clone(),
		committed: false,
	};
	lease(
		&mut scope,
		Uuid::new_v4(),
		30,
		None,
		repository.node_id(),
		&mut None,
	)
	.await
	.unwrap()
}
fn wrote_nothing(repository: &Repository) -> bool {
	let events = repository.0.events.lock().unwrap();
	!events.iter().any(|e| {
		matches!(
			e.as_str(),
			"lease" | "repair_lease" | "invalid_state" | "cancel_unloadable"
		)
	})
}

#[rstest]
#[tokio::test]
async fn an_old_worker_leaves_a_run_needing_tool_batch_state_untouched_until_a_capable_one_claims_it(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange: a new worker persisted state that only new workers can load.
	*repository.0.run.lock().unwrap() = Some(reserved(now));
	deploy(&repository, OLD_WORKER);

	// Act: the old worker keeps polling, by recovery scan and by notification.
	for _ in 0..3 {
		assert!(lease_as(&repository).await.is_none());
	}

	// Assert: it never leased, repaired or failed the Run, so no recovery attempt is burnt.
	assert!(wrote_nothing(&repository));

	// Act: the old worker is gone or upgraded.
	deploy(&repository, NEW_WORKER);
	let claimed = lease_as(&repository).await.unwrap();

	// Assert: the capable worker resumes it exactly once.
	assert_eq!(claimed.revision, 8);
	assert!(claimed.lease_owner.is_some());
}

#[rstest]
#[case::old_worker(OLD_WORKER)]
#[case::new_worker(NEW_WORKER)]
#[tokio::test]
async fn a_run_in_the_legacy_format_is_claimable_by_every_worker(
	repository: Repository,
	#[case] worker: &'static [PersistedCapability],
) {
	// Arrange: nothing in the persisted state needs a capability.
	assert!(
		encode(&RunState::Ready(ReadyState {}), &Default::default())
			.unwrap()
			.get("requires")
			.is_none()
	);
	deploy(&repository, worker);

	// Act
	let claimed = lease_as(&repository).await;

	// Assert
	assert_eq!(claimed.unwrap().revision, 8);
}

#[rstest]
#[tokio::test]
async fn a_notification_for_a_gated_run_backs_off_instead_of_renotifying_at_once(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange
	*repository.0.run.lock().unwrap() = Some(reserved(now));
	let envelope = repository
		.0
		.obligation
		.lock()
		.unwrap()
		.as_ref()
		.unwrap()
		.envelope(repository.node_id());
	deploy(&repository, OLD_WORKER);

	// Act
	let handoff = claim(&repository, &envelope).await.unwrap();

	// Assert: recorded and deferred a few seconds out; the Run is not leased or failed.
	assert!(matches!(handoff, Handoff::Recorded));
	assert!(wrote_nothing(&repository));
	assert_eq!(
		*repository.0.deferred.lock().unwrap(),
		[Some(
			now + chrono::Duration::seconds(CAPABILITY_RETRY_SECONDS)
		)]
	);

	// Act: the redelivered notification reaches a capable worker.
	deploy(&repository, NEW_WORKER);
	let handoff = claim(&repository, &envelope).await.unwrap();

	// Assert
	assert!(matches!(handoff, Handoff::Claimed(..)));
}

#[rstest]
#[case::added_field(serde_json::json!({"added_later": true}), 1)]
#[case::later_version(serde_json::json!({}), 2)]
#[tokio::test]
async fn state_this_build_cannot_decode_is_not_repaired_into_a_failed_run(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] data: serde_json::Value,
	#[case] version: u32,
) {
	// Arrange: a newer build wrote state this build cannot read without declaring it.
	let mut row = raw(now);
	row.pending["data"] = data;
	row.pending["state_version"] = version.into();
	*repository.0.run.lock().unwrap() = Some(row);

	// Act
	let claimed = lease_as(&repository).await;

	// Assert: a capability mismatch, not a corrupt Run.
	assert!(claimed.is_none());
	assert!(wrote_nothing(&repository));
}

#[rstest]
#[tokio::test]
async fn a_notification_for_undeclared_newer_state_backs_off_instead_of_renotifying_at_once(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange: no `requires`, but the state has a field this build does not know.
	let mut row = raw(now);
	row.pending["data"]["added_later"] = serde_json::json!(true);
	*repository.0.run.lock().unwrap() = Some(row);
	let envelope = repository
		.0
		.obligation
		.lock()
		.unwrap()
		.as_ref()
		.unwrap()
		.envelope(repository.node_id());

	// Act
	let handoff = claim(&repository, &envelope).await.unwrap();

	// Assert: deferred a few seconds out, like a declared requirement; nothing written.
	assert!(matches!(handoff, Handoff::Recorded));
	assert!(wrote_nothing(&repository));
	assert_eq!(
		*repository.0.deferred.lock().unwrap(),
		[Some(
			now + chrono::Duration::seconds(CAPABILITY_RETRY_SECONDS)
		)]
	);
}

#[rstest]
#[case::declared_requirement(reserved)]
#[case::added_field(|now| {
	let mut row = raw(now);
	row.pending["data"]["added_later"] = serde_json::json!(true);
	row
})]
#[case::later_version(|now| {
	let mut row = raw(now);
	row.pending["state_version"] = 2.into();
	row
})]
#[tokio::test]
async fn a_cancelled_run_is_finished_by_a_worker_that_cannot_load_its_state(
	repository: Repository,
	now: DateTime<Utc>,
	#[case] stored: fn(DateTime<Utc>) -> RawRun,
) {
	// Arrange: the Deployment Administrator cancelled a Run whose state no live worker can load.
	let mut row = stored(now);
	row.metadata.control = RunControl::Cancelled;
	*repository.0.run.lock().unwrap() = Some(row);
	deploy(&repository, OLD_WORKER);

	// Act
	let claimed = lease_as(&repository).await;

	// Assert: not leased or failed as corrupt; handed to cancellation delivery under a
	// repair lease, which needs only the Run's metadata.
	assert!(claimed.is_none());
	let events = repository.0.events.lock().unwrap();
	assert!(events.iter().any(|e| e == "repair_lease"));
	assert!(events.iter().any(|e| e == "cancel_unloadable"));
	assert!(
		!events
			.iter()
			.any(|e| matches!(e.as_str(), "lease" | "invalid_state"))
	);
}

#[rstest]
#[tokio::test]
async fn a_notification_for_a_cancelled_gated_run_finishes_the_cancellation_without_backing_off(
	repository: Repository,
	now: DateTime<Utc>,
) {
	// Arrange
	let mut row = reserved(now);
	row.metadata.control = RunControl::Cancelled;
	*repository.0.run.lock().unwrap() = Some(row);
	let envelope = repository
		.0
		.obligation
		.lock()
		.unwrap()
		.as_ref()
		.unwrap()
		.envelope(repository.node_id());
	deploy(&repository, OLD_WORKER);

	// Act
	let handoff = claim(&repository, &envelope).await.unwrap();

	// Assert: cancellation delivery took over and the obligation is not held back by the
	// capability retry delay.
	assert!(matches!(handoff, Handoff::Recorded));
	assert!(
		repository
			.0
			.events
			.lock()
			.unwrap()
			.iter()
			.any(|e| e == "cancel_unloadable")
	);
	assert_eq!(*repository.0.deferred.lock().unwrap(), [Some(now)]);
}
