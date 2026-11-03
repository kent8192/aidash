//! Real PostgreSQL contract: independent native replicas cannot silently repin a lost journal.
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};

#[rstest::rstest]
#[tokio::test]
async fn two_native_replicas_and_a_restart_retain_the_first_trusted_runner_instance(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let first_replica = database.pool().await;
	let second_replica = database.pool().await;
	let deployment =
		aidash_domain::nodes::DeploymentId::parse("aidash://allocation-journal-test").unwrap();
	let one = Uuid::new_v4();
	let two = Uuid::new_v4();
	let (first, second) = tokio::join!(
		confirm(&first_replica, &deployment, one),
		confirm(&second_replica, &deployment, two),
	);
	assert_ne!(
		first.is_ok(),
		second.is_ok(),
		"exactly one first pin wins CAS"
	);
	let (winner, rejected) = if first.is_ok() {
		(one, two)
	} else {
		(two, one)
	};
	drop(first_replica);
	drop(second_replica);
	let restarted = database.pool().await;
	confirm(&restarted, &deployment, winner).await.unwrap();
	assert!(matches!(
		confirm(&restarted, &deployment, rejected).await,
		Err(Error::External(_))
	));
	// A failure cannot mutate the first pin; it remains recoverable by restoring that journal.
	confirm(&restarted, &deployment, winner).await.unwrap();
}
