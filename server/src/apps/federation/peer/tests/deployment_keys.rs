//! Deterministic stale-snapshot checks at the real PostgreSQL CAS boundary.
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use aidash_application::federation::key::{
	DeploymentKey, DeploymentKeySource, ScheduledDeploymentKeys,
};
use aidash_integrations::federation::key::{FileDeploymentKey, FileKeySources};
use std::sync::Arc;

#[rstest::rstest]
#[tokio::test]
async fn postgres_schedule_cannot_insert_over_or_replace_a_newer_snapshot(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let dir = tempfile::tempdir().unwrap();
	let initial_path = dir.path().join("first.pem");
	let second_path = dir.path().join("second.pem");
	let third_path = dir.path().join("third.pem");
	let initial = FileDeploymentKey::generate(&initial_path, false).unwrap();
	let second = FileDeploymentKey::generate(&second_path, false).unwrap();
	FileDeploymentKey::generate(&third_path, false).unwrap();
	let store = Arc::new(PostgresDeploymentKeys {
		pool: database.pool().await,
		deployment: DeploymentId::parse("aidash://sender").unwrap(),
	});
	let ring = ScheduledDeploymentKeys::new(
		store.deployment.clone(),
		DeploymentKeySource::File(initial_path),
		store.clone(),
		Arc::new(FileKeySources),
	);
	let now = 1_800_000_000;
	ring.rollover(
		initial.key_id(),
		DeploymentKeySource::File(second_path),
		now,
	)
	.await
	.unwrap();
	let first = store.load().await.unwrap().unwrap();
	assert!(
		!store.compare_and_set(None, &first).await.unwrap(),
		"an existing row is not an empty snapshot"
	);
	ring.rollover(
		second.key_id(),
		DeploymentKeySource::File(third_path),
		now + 691_200,
	)
	.await
	.unwrap();
	let latest = store.load().await.unwrap().unwrap();
	assert!(
		!store.compare_and_set(Some(&first), &first).await.unwrap(),
		"stale expected identity cannot roll the schedule back"
	);
	assert_eq!(store.load().await.unwrap(), Some(latest));
}
