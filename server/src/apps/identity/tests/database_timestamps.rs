use super::{DashboardIdentity, DashboardSession};
use crate::apps::execution::test_database::{DatabaseFixture, database};
use chrono::{DateTime, Duration, Utc};
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
		.valid_since(None)
		.verified_email(None)
		.display_name(None)
		.last_valid_at(None)
		.disabled_at(Some(expected))
		.display_observed_at(None)
		.display_erased_at(None)
		.display_erased_by(None)
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

#[rstest]
#[tokio::test]
async fn display_retention_finds_only_a_live_session_with_its_own_idle_limit(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let mut connection = database.lease.handle();
	let now = Utc::now();
	let identity = DashboardIdentity::build()
		.issuer("fixture")
		.subject("session-retention")
		.valid_since(None)
		.verified_email(Some("person@example.test".into()))
		.display_name(Some("Person".into()))
		.last_valid_at(None)
		.disabled_at(None)
		.display_observed_at(None)
		.display_erased_at(None)
		.display_erased_by(None)
		.finish();
	DashboardIdentity::objects()
		.create_with_conn(&mut connection, &identity)
		.await
		.unwrap();

	// These sessions are unrevoked and not absolutely expired, but all exceed
	// the default idle limit. They must not retain display attributes.
	for index in 0_u8..16 {
		let session = DashboardSession::build()
			.id(uuid::Uuid::new_v4())
			.token_hash(vec![index])
			.csrf_hash(vec![index])
			.identity(identity.id)
			.provider_sid(None)
			.created_at(now - Duration::hours(2))
			.auth_time(None)
			.last_activity_at(now - Duration::minutes(45))
			.expires_at(now + Duration::hours(1))
			.revoked_at(None)
			.desktop(false)
			.desktop_idle_seconds(None)
			.access_expires_at(None)
			.finish();
		DashboardSession::objects()
			.create_with_conn(&mut connection, &session)
			.await
			.unwrap();
	}
	assert!(
		!connection
			.atomic(async |tx| DashboardIdentity::retains_display(tx, &identity, Some(1800)).await)
			.await
			.unwrap()
	);

	// A desktop-specific 60-minute limit makes this session the sole live one.
	let live = DashboardSession::build()
		.id(uuid::Uuid::new_v4())
		.token_hash(vec![255])
		.csrf_hash(vec![255])
		.identity(identity.id)
		.provider_sid(None)
		.created_at(now - Duration::hours(2))
		.auth_time(None)
		.last_activity_at(now - Duration::minutes(45))
		.expires_at(now + Duration::hours(1))
		.revoked_at(None)
		.desktop(true)
		.desktop_idle_seconds(Some(3600))
		.access_expires_at(None)
		.finish();
	DashboardSession::objects()
		.create_with_conn(&mut connection, &live)
		.await
		.unwrap();
	assert!(
		connection
			.atomic(async |tx| DashboardIdentity::retains_display(tx, &identity, Some(1800)).await)
			.await
			.unwrap()
	);
	DashboardSession::objects()
		.filter(DashboardSession::field_id().eq(live.id))
		.update_fields_with_conn(
			&mut connection,
			[(DashboardSession::field_revoked_at(), Some(now))],
		)
		.await
		.unwrap();
	assert!(
		!connection
			.atomic(async |tx| DashboardIdentity::retains_display(tx, &identity, Some(1800)).await)
			.await
			.unwrap()
	);
}
