//! The server's database login (#262): a login that row-level security would
//! not bind is refused, and session state never passes between pool borrowers.
use super::native_database::{ADMIN, DatabaseFixture, OWNER, database, with_login};
use aidash_server::database::runtime;
use rstest::rstest;

#[rstest]
#[case::superuser(ADMIN, "is a superuser")]
// The owner role owns the database as well as every table, as in a deployment.
#[case::owner(OWNER, "owns the database")]
#[tokio::test]
async fn server_refuses_a_login_that_bypasses_row_level_security(
	#[future] database: DatabaseFixture,
	#[case] login: (&str, &str),
	#[case] reason: &str,
) {
	// Arrange: the fixture migrated as the owner, as a deployment's Job does.
	let f = database.await;
	let url = with_login(&f.url, login);
	// Act
	let refused = runtime::connect(&url, 2).await;
	// Assert
	let error = refused.err().expect("a privileged login is refused");
	assert!(error.to_string().contains(reason), "{error}");
}

/// A login that owns no table but owns a schema can still create and drop
/// objects in it, so it is refused as well.
#[rstest]
#[tokio::test]
async fn server_refuses_a_login_that_owns_a_schema(#[future] database: DatabaseFixture) {
	// Arrange: role and schema DDL that Reinhardt Query cannot express.
	let f = database.await;
	let admin = sqlx::PgPool::connect(&with_login(&f.url, ADMIN))
		.await
		.unwrap();
	for statement in [
		"CREATE ROLE schema_probe LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD 'probe-password'",
		"CREATE SCHEMA probe AUTHORIZATION schema_probe",
	] {
		sqlx::query(statement).execute(&admin).await.unwrap();
	}
	admin.close().await;
	let url = with_login(&f.url, ("schema_probe", "probe-password"));
	// Act
	let refused = runtime::connect(&url, 2).await;
	// Assert
	let error = refused.err().expect("a schema owner is refused");
	assert!(error.to_string().contains("owns a schema"), "{error}");
}

/// Membership in the owning role carries its ownership, through inheritance
/// or `SET ROLE`, so a member of `aidash_owner` is refused like the owner.
#[rstest]
#[case::inherited("INHERIT")]
#[case::set_role_only("NOINHERIT")]
#[tokio::test]
async fn server_refuses_a_member_of_the_owner_role(
	#[future] database: DatabaseFixture,
	#[case] inheritance: &str,
) {
	// Arrange: role DDL that Reinhardt Query cannot express.
	let f = database.await;
	let admin = sqlx::PgPool::connect(&with_login(&f.url, ADMIN))
		.await
		.unwrap();
	for statement in [
		format!(
			"CREATE ROLE member_probe LOGIN {inheritance} NOSUPERUSER NOBYPASSRLS PASSWORD 'probe-password'"
		),
		format!("GRANT {} TO member_probe", OWNER.0),
	] {
		sqlx::query(&statement).execute(&admin).await.unwrap();
	}
	admin.close().await;
	let url = with_login(&f.url, ("member_probe", "probe-password"));
	// Act
	let refused = runtime::connect(&url, 2).await;
	// Assert
	let error = refused
		.err()
		.expect("a member of the owner role is refused");
	assert!(error.to_string().contains("owns the database"), "{error}");
}

#[rstest]
#[tokio::test]
async fn server_role_holds_only_data_privileges(#[future] database: DatabaseFixture) {
	// Arrange
	let f = database.await;
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	// Act: schema changes and TRUNCATE, which skips row-level security.
	let create = sqlx::query("CREATE TABLE role_probe (id integer)")
		.execute(&pool)
		.await;
	let truncate = sqlx::query("TRUNCATE inbox").execute(&pool).await;
	let select = sqlx::query("SELECT count(*) FROM inbox")
		.execute(&pool)
		.await;
	// Assert
	for refused in [create, truncate] {
		let error = refused.unwrap_err();
		assert_eq!(
			error
				.as_database_error()
				.and_then(|error| error.code())
				.as_deref(),
			Some("42501"),
			"{error}"
		);
	}
	select.unwrap();
}

#[rstest]
#[tokio::test]
async fn returned_connections_lose_session_settings(#[future] database: DatabaseFixture) {
	// Arrange: one connection, so every borrow reuses it.
	let f = database.await;
	let pool = runtime::connect(&f.url, 1)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	sqlx::query("SELECT set_config('aidash.probe', 'leaked', false)")
		.execute(&pool)
		.await
		.unwrap();
	// An abandoned transaction is rolled back before the reset runs.
	let abandoned = pool.begin().await.unwrap();
	drop(abandoned);
	// Act
	let value: Option<String> = sqlx::query_scalar("SELECT current_setting('aidash.probe', true)")
		.fetch_one(&pool)
		.await
		.unwrap();
	// Assert: the placeholder may remain defined, but only with an empty value.
	assert_eq!(value.unwrap_or_default(), "");
}
