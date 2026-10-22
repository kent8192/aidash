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

/// The migration Job runs maintenance commands such as `demo-data` as the
/// owner. Row-level security is forced on the owner too, so those commands may
/// connect as it, but a login that bypasses the policy is refused as before.
#[rstest]
#[tokio::test]
async fn maintenance_accepts_the_owner_but_not_a_login_that_bypasses_row_level_security(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let f = database.await;
	let owner = with_login(&f.url, OWNER);
	// Act / Assert: the server's own connection refuses the owner.
	let refused = runtime::connect(&owner, 2).await;
	assert!(refused.is_err());
	// Act / Assert: the maintenance connection accepts it. Row-level security
	// binding the owner is proved in `row_level_security`.
	let connection = runtime::connect_maintenance(&owner, 2)
		.await
		.expect("the migration owner may run maintenance commands");
	drop(connection);
	// Act / Assert: a superuser bypasses the policy and is refused.
	let refused = runtime::connect_maintenance(&with_login(&f.url, ADMIN), 2).await;
	let error = refused.err().expect("a superuser is refused");
	assert!(error.to_string().contains("is a superuser"), "{error}");
}

/// The migration Job runs `memory-recovery init-if-missing` before its
/// `demo-data` step under the owner's login. That command must accept the
/// owner, or the Job's `-e` shell exits before Demo data is loaded.
#[rstest]
#[tokio::test]
async fn memory_recovery_init_if_missing_accepts_the_migration_owner(
	#[future] database: DatabaseFixture,
) {
	// Arrange: the fixture is migrated and hands over the owner URL exactly as
	// the Job's `app-migration` Secret does.
	let f = database.await;
	let directory = tempfile::tempdir().unwrap();
	let home = directory.path().join("home");
	std::fs::create_dir(&home).unwrap();
	let mut command = crate::deployment::deployment_command(&f.owner_url, directory.path());
	command
		.args(["memory-recovery", "init-if-missing", "--directory"])
		.arg(&home);
	// Act
	let output = command.output().await.expect("run memory-recovery");
	// Assert: the owner is accepted, so the Job reaches the appended command.
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	assert!(home.join("epoch.cbor").exists());
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

/// Privileges a login can gain without owning a schema or relation are refused
/// too: REPLICATION, SET-capable membership in a BYPASSRLS or REPLICATION role,
/// membership in a role that reaches the server's files or programs, CREATE,
/// TRUNCATE or TRIGGER held directly or through a role reachable only by SET
/// ROLE, ownership of a function, and a startup role that hides the
/// authenticated login.
#[rstest]
#[case::replication(&["ALTER ROLE grant_probe REPLICATION"], "has REPLICATION")]
#[case::bypassrls_member(
	&["CREATE ROLE bypass_probe NOLOGIN BYPASSRLS", "GRANT bypass_probe TO grant_probe WITH INHERIT FALSE, SET TRUE"],
	"can SET ROLE to a superuser, BYPASSRLS or REPLICATION role"
)]
#[case::replication_member(
	&["CREATE ROLE replication_probe NOLOGIN REPLICATION", "GRANT replication_probe TO grant_probe WITH INHERIT FALSE, SET TRUE"],
	"can SET ROLE to a superuser, BYPASSRLS or REPLICATION role"
)]
#[case::server_program(
	&["CREATE ROLE program_probe NOLOGIN", "GRANT pg_execute_server_program TO program_probe", "GRANT program_probe TO grant_probe"],
	"reaches the server's files or programs"
)]
#[case::database_create(
	&["DO $$ BEGIN EXECUTE format('GRANT CREATE ON DATABASE %I TO grant_probe', current_database()); END $$"],
	"can create"
)]
#[case::schema_create(&["GRANT CREATE ON SCHEMA public TO grant_probe"], "can create")]
#[case::schema_create_by_set_role(
	&["CREATE ROLE create_probe NOLOGIN", "GRANT CREATE ON SCHEMA public TO create_probe", "GRANT create_probe TO grant_probe WITH INHERIT FALSE, SET TRUE"],
	"can create"
)]
#[case::truncate(&["GRANT TRUNCATE ON inbox TO grant_probe"], "can truncate tables")]
#[case::trigger(&["GRANT TRIGGER ON inbox TO grant_probe"], "create triggers")]
#[case::truncate_by_set_role(
	&["CREATE ROLE truncate_probe NOLOGIN", "GRANT TRUNCATE ON inbox TO truncate_probe", "GRANT truncate_probe TO grant_probe WITH INHERIT FALSE, SET TRUE"],
	"can truncate tables"
)]
#[case::function_owner(
	&["CREATE FUNCTION function_probe() RETURNS integer LANGUAGE sql AS 'SELECT 1'", "ALTER FUNCTION function_probe() OWNER TO grant_probe"],
	"owns database functions"
)]
#[case::startup_role(
	&["GRANT aidash_server TO grant_probe", "ALTER ROLE grant_probe SET role = 'aidash_server'"],
	"starts as another role"
)]
#[tokio::test]
async fn server_refuses_a_login_with_indirect_privileges(
	#[future] database: DatabaseFixture,
	#[case] grants: &[&str],
	#[case] reason: &str,
) {
	// Arrange: role DDL that Reinhardt Query cannot express.
	let f = database.await;
	let admin = sqlx::PgPool::connect(&with_login(&f.url, ADMIN))
		.await
		.unwrap();
	let create = "CREATE ROLE grant_probe LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD 'probe-password'";
	for statement in std::iter::once(create).chain(grants.iter().copied()) {
		sqlx::query(statement).execute(&admin).await.unwrap();
	}
	admin.close().await;
	let url = with_login(&f.url, ("grant_probe", "probe-password"));
	// Act
	let refused = runtime::connect(&url, 2).await;
	// Assert
	let error = refused
		.err()
		.expect("a login with indirect privileges is refused");
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

/// The owner's default privileges grant the server role DML on the migration
/// ledger as well, but startup trusts the ledger, so `aidash migrate` leaves
/// it read-only to the server.
#[rstest]
#[tokio::test]
async fn migrate_leaves_the_ledger_read_only_to_the_server(#[future] database: DatabaseFixture) {
	// Arrange: the fixture applied migrations through the executor alone.
	let f = database.await;
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	let writable = "SELECT has_table_privilege('reinhardt_migrations', 'DELETE')";
	let before: bool = sqlx::query_scalar(writable).fetch_one(&pool).await.unwrap();
	assert!(before, "default privileges grant the ledger to the server");
	let mut context = reinhardt::commands::CommandContext::default();
	context.set_option("database".into(), with_login(&f.url, OWNER));
	// Act: nothing is pending, so `migrate` only seals the ledger.
	aidash_server::bootstrap::migrations::run(&context)
		.await
		.unwrap();
	// Assert
	for statement in [
		"INSERT INTO reinhardt_migrations (app) VALUES ('probe')",
		"UPDATE reinhardt_migrations SET app = app",
		"DELETE FROM reinhardt_migrations",
		"TRUNCATE reinhardt_migrations",
	] {
		let error = sqlx::query(statement).execute(&pool).await.unwrap_err();
		assert_eq!(
			error
				.as_database_error()
				.and_then(|error| error.code())
				.as_deref(),
			Some("42501"),
			"{statement}: {error}"
		);
	}
	sqlx::query("SELECT count(*) FROM reinhardt_migrations")
		.execute(&pool)
		.await
		.unwrap();
}

/// Migrations commit one by one, so a run that fails part-way leaves ledger
/// records behind; `migrate` seals the ledger on that path as well.
#[rstest]
#[tokio::test]
async fn a_failed_migrate_still_leaves_the_ledger_read_only(#[future] database: DatabaseFixture) {
	// Arrange: the native history plus one migration that fails.
	let f = database.await;
	let directory = tempfile::TempDir::new().unwrap();
	copy_tree(
		&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
		directory.path(),
	);
	std::fs::write(
		directory.path().join("marketplace/0008_failing_probe.rs"),
		r#"// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0008_failing_probe", "marketplace")
		.add_dependency("marketplace", "0007_model_state")
		.add_operation(Operation::RunSQL {
			sql: "SELECT 1 FROM failing_probe_missing_table".to_string(),
			reverse_sql: None,
		})
}
"#,
	)
	.unwrap();
	let mut context = reinhardt::commands::CommandContext::default();
	context.set_option("database".into(), with_login(&f.url, OWNER));
	context.set_option(
		"migrations-dir".into(),
		directory.path().to_string_lossy().into_owned(),
	);
	// Act
	let failed = aidash_server::bootstrap::migrations::run(&context).await;
	// Assert: the migration's own error surfaces, and the ledger is sealed.
	let error = failed.expect_err("the probe migration fails");
	assert!(
		error.to_string().contains("failing_probe_missing_table"),
		"{error}"
	);
	let pool = runtime::connect(&f.url, 2)
		.await
		.unwrap()
		.into_postgres()
		.unwrap();
	let deleted = sqlx::query("DELETE FROM reinhardt_migrations")
		.execute(&pool)
		.await
		.unwrap_err();
	assert_eq!(
		deleted
			.as_database_error()
			.and_then(|error| error.code())
			.as_deref(),
		Some("42501"),
		"{deleted}"
	);
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
	std::fs::create_dir_all(to).unwrap();
	for entry in std::fs::read_dir(from).unwrap() {
		let entry = entry.unwrap();
		let target = to.join(entry.file_name());
		if entry.file_type().unwrap().is_dir() {
			copy_tree(&entry.path(), &target);
		} else {
			std::fs::copy(entry.path(), target).unwrap();
		}
	}
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
