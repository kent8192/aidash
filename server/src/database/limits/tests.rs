use super::*;
use rstest::rstest;

#[rstest]
#[case("-c statement_timeout=300000ms", Some(Duration::from_secs(330)))]
#[case(
	"-c tcp_keepalives_idle=15 -c statement_timeout=2000ms -c tcp_user_timeout=30000",
	Some(Duration::from_secs(32))
)]
#[case(
	"-c statement_timeout=1min -c statement_timeout=2000ms",
	Some(Duration::from_secs(32))
)]
#[case("-c statement_timeout=0ms", None)]
#[case("-c tcp_keepalives_idle=15", None)]
fn the_client_deadline_follows_the_limit_on_the_pool(
	#[case] options: &str,
	#[case] expected: Option<Duration>,
) {
	// Arrange
	let options = PgConnectOptions::new().options(
		options
			.split_whitespace()
			.filter(|part| *part != "-c")
			.filter_map(|part| part.split_once('=')),
	);
	// Act and assert
	assert_eq!(deadline(&options), expected);
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn a_statement_with_no_answer_fails_at_the_deadline_as_a_temporary_database_error() {
	// Arrange
	let deadline = Duration::from_secs(330);
	let started = tokio::time::Instant::now();
	// Act
	let result: DriverResult<()> = bounded(deadline, std::future::pending()).await;
	// Assert
	assert_eq!(started.elapsed(), deadline);
	assert!(crate::Error::from(result.unwrap_err()).is_transient_database());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn an_exempt_statement_has_no_deadline() {
	// Arrange
	let started = tokio::time::Instant::now();
	let long = Duration::from_secs(10_000);
	// Act
	let result: DriverResult<u8> = UNLIMITED
		.scope(
			(),
			bounded(Duration::from_secs(1), async move {
				tokio::time::sleep(long).await;
				Ok(7)
			}),
		)
		.await;
	// Assert
	assert_eq!(result.unwrap(), 7);
	assert_eq!(started.elapsed(), long);
}
