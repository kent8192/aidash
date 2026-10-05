use super::DashboardIdentity;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Model;
use rstest::rstest;

#[rstest]
#[case::after_epoch("2026-10-05T01:02:03.123456789Z", "2026-10-05T01:02:03.123456Z")]
#[case::before_epoch("1999-12-31T23:59:59.999999501Z", "2000-01-01T00:00:00Z")]
#[case::before_unix_epoch("1969-12-31T23:59:59.123456789Z", "1969-12-31T23:59:59.123457Z")]
#[tokio::test]
async fn identity_restore_accepts_host_clock_precision_at_the_orm_boundary(
	#[future] database: DatabaseFixture,
	#[case] checked_at: &str,
	#[case] expected: &str,
) {
	// Arrange
	let database = database.await;
	let mut connection = database.lease.handle();
	let checked_at = DateTime::parse_from_rfc3339(checked_at)
		.unwrap()
		.with_timezone(&Utc);
	let expected = DateTime::parse_from_rfc3339(expected)
		.unwrap()
		.with_timezone(&Utc);
	let identity = DashboardIdentity::build()
		.issuer("fixture")
		.subject("nanosecond-clock")
		.last_valid_at(None)
		.disabled_at(Some(expected))
		.finish();
	DashboardIdentity::objects()
		.create_with_conn(&mut connection, &identity)
		.await
		.unwrap();
	// Act
	DashboardIdentity::restore(&mut connection, identity.id, checked_at)
		.await
		.unwrap();
	// Assert
	let saved = DashboardIdentity::objects()
		.filter(DashboardIdentity::field_id().eq(identity.id))
		.get_with_db(&mut connection)
		.await
		.unwrap();
	assert_eq!(saved.last_valid_at, Some(expected));
	assert_eq!(saved.disabled_at, None);
}
