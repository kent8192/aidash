//! The server's database login (#262): a login that row-level security would
//! not bind is refused, and session state never passes between pool borrowers.
use super::native_database::{ADMIN, DatabaseFixture, OWNER, database, with_login};
use aidash_server::database::runtime;
use rstest::rstest;

#[rstest]
#[case::superuser(ADMIN, "is a superuser")]
#[case::table_owner(OWNER, "owns database objects")]
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
