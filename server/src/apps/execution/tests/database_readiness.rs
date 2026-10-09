//! Exercise readiness policy through real PostgreSQL errors and backend sessions.
use super::native_database::{postgres_container, wait_for_pgroonga};
use reinhardt::query::types::function::FunctionLanguage;
use reinhardt::query::{Alias, PostgresQueryBuilder, Query, QueryStatementBuilder};
use reinhardt::test::testcontainers::{ContainerAsync, GenericImage};
use rstest::rstest;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

#[rstest]
#[case::already_ready(0, "57P03", "pgroonga: pgroonga_crash_safer is preparing", false, None)]
#[case::preparing_then_ready(
	1,
	"57P03",
	"pgroonga: pgroonga_crash_safer is preparing",
	false,
	None
)]
#[case::permanent_error(
	1,
	"42501",
	"permission denied for function pgroonga_command",
	false,
	Some("42501")
)]
#[case::preparing_message_with_permanent_code(
	1,
	"XX000",
	"pgroonga: pgroonga_crash_safer is preparing",
	false,
	Some("XX000")
)]
#[case::unrelated_cannot_connect(
	1,
	"57P03",
	"database is unavailable",
	false,
	Some("database is unavailable")
)]
#[case::preparing_deadline(
	1_000_000,
	"57P03",
	"pgroonga: pgroonga_crash_safer is preparing",
	false,
	Some("last transient error: SQLSTATE 57P03")
)]
#[case::stalled_probe(
	0,
	"57P03",
	"pgroonga: pgroonga_crash_safer is preparing",
	true,
	Some("probe did not complete")
)]
#[tokio::test]
async fn pgroonga_readiness_obeys_retry_policy_and_deadline(
	#[future] postgres_container: (ContainerAsync<GenericImage>, Arc<PgPool>, u16, String),
	#[case] preparing_attempts: i64,
	#[case] code: &str,
	#[case] message: &str,
	#[case] stalled: bool,
	#[case] expected_error: Option<&str>,
) {
	// Arrange: sequence increments survive raised errors, so they count actual
	// attempts rather than committed statements.
	let (_container, pool, _, _) = postgres_container.await;
	for name in ["readiness_attempts", "readiness_first_backend"] {
		let ddl = Query::create_sequence()
			.name(name)
			.to_string(PostgresQueryBuilder);
		sqlx::query(&ddl).execute(pool.as_ref()).await.unwrap();
	}
	let body = format!(
		r#"
		DECLARE
			attempt bigint := nextval('readiness_attempts');
		BEGIN
			IF attempt = 1 THEN
				PERFORM setval('readiness_first_backend', pg_backend_pid());
			ELSIF pg_backend_pid() = (SELECT last_value FROM readiness_first_backend) THEN
				RAISE EXCEPTION 'readiness reused an invalidated backend';
			END IF;
			IF {stalled} THEN
				PERFORM pg_sleep(2);
			END IF;
			IF attempt <= {preparing_attempts} THEN
				RAISE EXCEPTION USING ERRCODE = '{code}', MESSAGE = '{message}';
			END IF;
			RETURN 'ready';
		END;
		"#
	);
	let ddl = Query::create_function()
		.name("pgroonga_command")
		.add_parameter("command", "text")
		.returns("text")
		.language(FunctionLanguage::PlPgSql)
		.body(body)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&ddl).execute(pool.as_ref()).await.unwrap();
	let timeout = Duration::from_millis(350);

	// Act: use the same probe, error classification and session disposal as restart.
	let started = std::time::Instant::now();
	let result = tokio::time::timeout(Duration::from_secs(2), wait_for_pgroonga(&pool, timeout))
		.await
		.expect("fixture readiness must stop even when its probe never becomes ready");
	let elapsed = started.elapsed();

	// Assert: readiness, permanent failures and deadline exhaustion stay distinct.
	match expected_error {
		None => result.unwrap(),
		Some(expected) => {
			let error = result.unwrap_err();
			assert!(error.contains(expected), "{error}");
			if preparing_attempts > 1 || stalled {
				assert!(error.contains("350ms"), "{error}");
				assert!(elapsed >= timeout);
			}
		}
	}
	assert!(
		elapsed < Duration::from_secs(2),
		"readiness took {elapsed:?}"
	);
	// Read last_value rather than currval: the inspection connection is independent.
	let counter = Query::select()
		.column(Alias::new("last_value"))
		.from(Alias::new("readiness_attempts"))
		.to_string(PostgresQueryBuilder);
	let attempts: i64 = sqlx::query_scalar(&counter)
		.fetch_one(pool.as_ref())
		.await
		.unwrap();
	if preparing_attempts > 1 {
		assert!(
			attempts >= 2,
			"preparing was not retried: {attempts} attempts"
		);
	} else {
		let expected_attempts = if expected_error.is_none() && preparing_attempts == 1 {
			2
		} else {
			1
		};
		assert_eq!(attempts, expected_attempts);
	}
}
