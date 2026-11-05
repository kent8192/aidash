//! The statement limit, through real PostgreSQL errors (#728).
use super::{DatabaseFixture, database};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_server::database::{limits::execute_unlimited, native};
use reinhardt::db::backends::TransactionExecutor;
use rstest::rstest;

const SCOPE: TenancyScope = TenancyScope::Deployment(DeploymentScopeReason::SemanticMaintenance);
const SLEEP: &str = "SELECT pg_sleep(0.5)";

fn limited(url: &str) -> native::Pool {
	let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
	sqlx::postgres::PgPoolOptions::new()
		.max_connections(2)
		.connect_lazy_with(options.options([("statement_timeout", "100ms")]))
		.into()
}

#[rstest]
#[tokio::test]
async fn a_statement_past_the_limit_is_a_temporary_database_error(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let database = database.await;
	let pool = limited(&database.url);
	let mut tx = pool.begin(&SCOPE).await.unwrap();
	// Act
	let error = aidash_server::Error::from(tx.execute(SLEEP, vec![]).await.unwrap_err());
	// Assert
	let application = aidash_application::Error::from(error);
	assert!(
		matches!(application, aidash_application::Error::TemporaryDatabase(_)),
		"{application}"
	);
	assert_eq!(
		aidash_server::Error::from(application)
			.http_response()
			.status
			.as_u16(),
		503
	);
}

#[rstest]
#[tokio::test]
async fn an_exempt_statement_runs_past_the_limit_and_the_limit_returns_after_it(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let database = database.await;
	let pool = limited(&database.url);
	let mut tx = pool.begin(&SCOPE).await.unwrap();
	let executor: &mut dyn TransactionExecutor = &mut *tx;
	// Act
	let exempt = execute_unlimited(&mut *executor, SLEEP, vec![]).await;
	let after = executor.execute(SLEEP, vec![]).await;
	// Assert
	exempt.unwrap();
	assert!(after.is_err());
}

#[rstest]
#[tokio::test]
async fn the_limit_survives_a_connection_being_returned_to_the_pool(
	#[future] database: DatabaseFixture,
) {
	// Arrange: `after_release` runs RESET ALL on the server's pools.
	let database = database.await;
	let options: sqlx::postgres::PgConnectOptions = database.url.parse().unwrap();
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(1)
		.after_release(|connection, _| {
			Box::pin(async move {
				use sqlx::Executor as _;
				connection.execute("RESET ALL").await?;
				Ok(true)
			})
		})
		.connect_lazy_with(options.options([("statement_timeout", "100ms")]));
	sqlx::query("SELECT 1").execute(&pool).await.unwrap();
	// Act
	let result = sqlx::query(SLEEP).execute(&pool).await;
	// Assert
	assert_eq!(
		result
			.unwrap_err()
			.as_database_error()
			.unwrap()
			.code()
			.unwrap(),
		"57014"
	);
}
