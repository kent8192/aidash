use super::Error;
use reinhardt::core::exception::{DatabaseError, DatabaseErrorKind};
use rstest::{fixture, rstest};
use serde_json::Value;

#[fixture]
fn database_error() -> DatabaseError {
	DatabaseError::new(DatabaseErrorKind::Query, "private database diagnostic")
}

#[rstest]
#[case("55P03", 503, Some("x-aidash-transaction-pending"), true)]
#[case("A3301", 409, Some("x-aidash-run-message-pending"), true)]
#[case("40001", 503, None, true)]
#[case("57014", 503, None, true)]
#[case("23514", 500, None, false)]
fn native_database_errors_preserve_retry_signals_without_exposing_diagnostics(
	database_error: DatabaseError,
	#[case] code: &str,
	#[case] status: u16,
	#[case] header: Option<&str>,
	#[case] retryable: bool,
) {
	// Arrange
	let error = Error::Framework(database_error.with_code(code).into());
	assert_eq!(error.is_transient_database(), retryable);
	// Act
	let response = error.http_response();
	// Assert
	assert_eq!(response.status.as_u16(), status);
	if let Some(header) = header {
		assert_eq!(response.headers[header], "1");
	}
	assert_eq!(response.headers.contains_key("retry-after"), status == 503);
	let body: Value = serde_json::from_slice(&response.body).unwrap();
	assert!(!body.to_string().contains("private database diagnostic"));
}

#[rstest]
#[case(DatabaseErrorKind::CheckViolation)]
#[case(DatabaseErrorKind::NotNullViolation)]
fn persistence_constraints_keep_the_legacy_server_error_envelope(#[case] kind: DatabaseErrorKind) {
	// Arrange: these kinds carry a framework HTTP 400 default.
	let error = Error::Framework(DatabaseError::new(kind, "private constraint detail").into());
	// Act
	let response = error.http_response();
	// Assert: an unexpected persistence failure keeps Aidash's existing contract.
	assert_eq!(response.status.as_u16(), 500);
	assert_eq!(
		serde_json::from_slice::<Value>(&response.body).unwrap(),
		serde_json::json!({"error":"operation failed; see server logs"})
	);
	assert!(!response.headers.contains_key("retry-after"));
}

#[rstest]
#[case(crate::semantic::remote::Failure::Pending, 503, "pending")]
#[case(crate::semantic::remote::Failure::Unavailable, 503, "unavailable")]
#[case(crate::semantic::remote::Failure::Authority, 409, "authority")]
#[case(crate::semantic::remote::Failure::Stale, 409, "stale")]
#[case(
	crate::semantic::remote::Failure::DurableMemoryBudget,
	409,
	"context_budget"
)]
fn semantic_failures_preserve_status_and_retry_headers(
	#[case] reason: crate::semantic::remote::Failure,
	#[case] status: u16,
	#[case] code: &str,
) {
	let error = Error::RemoteSemantic(reason);
	let expected = error.to_string();
	let response = error.http_response();
	assert_eq!(response.status.as_u16(), status);
	assert_eq!(response.headers["x-aidash-semantic-reason"], code);
	assert_eq!(
		response
			.headers
			.get("x-aidash-semantic-cause")
			.map(|value| value.to_str().unwrap()),
		(reason == crate::semantic::remote::Failure::DurableMemoryBudget)
			.then_some("durable_memory_budget")
	);
	assert_eq!(response.headers.contains_key("retry-after"), status == 503);
	let body: Value = serde_json::from_slice(&response.body).unwrap();
	assert_eq!(body["error"], expected);
}

#[rstest]
#[case::index(false)]
#[case::source(true)]
fn persisted_semantic_decode_errors_keep_the_internal_error_boundary(#[case] source: bool) {
	// Arrange: neither saved data failure is a client-supplied parameter failure.
	let error = if source {
		aidash_domain::semantic::indexing::source(
			&aidash_domain::semantic::indexing::IndexingEntry {
				id: uuid::Uuid::nil(),
				workspace_id: uuid::Uuid::nil(),
				source: serde_json::json!({"kind":"memory","private":"saved source diagnostic"}),
				revision: 1,
				point_id: uuid::Uuid::nil(),
				state: "PENDING".into(),
				attempts: 0,
			},
		)
		.unwrap_err()
	} else {
		aidash_domain::semantic::mutations::Index {
			workspace_id: uuid::Uuid::nil(),
			tenant: "tenant".into(),
			revision: 1,
			spec: serde_json::json!({"private":"saved index diagnostic"}),
			collection: "collection".into(),
			updated_at: chrono::Utc::now(),
		}
		.configuration()
		.unwrap_err()
	};
	// Act: exercise the use-case error conversion and Reinhardt response together.
	let error = Error::from(aidash_application::Error::from(error));
	assert!(matches!(error, Error::Json(_)));
	let response = error.http_response();
	// Assert: the public envelope stays identical and contains no saved diagnostic.
	assert_eq!(response.status.as_u16(), 500);
	assert_eq!(
		serde_json::from_slice::<Value>(&response.body).unwrap(),
		serde_json::json!({"error":"operation failed; see server logs"})
	);
	assert!(!response.headers.contains_key("retry-after"));
}

#[rstest]
fn stalled_streams_retry_like_provider_transport_timeouts() {
	// Arrange
	let stalled = Error::from(aidash_application::Error::InferenceStalled);
	let timeout = Error::from(aidash_application::Error::External(
		"model inference timed out".into(),
	));
	// Act
	let classify = crate::apps::execution::repositories::worker::classify_failure;
	// Assert
	assert!(matches!(stalled, Error::InferenceStalled));
	assert!(classify(&stalled).retryable());
	assert_eq!(
		classify(&stalled).retryable(),
		classify(&timeout).retryable()
	);
	assert!(matches!(
		aidash_application::Error::from(stalled),
		aidash_application::Error::InferenceStalled
	));
}

/// Issue #233: a local semantic outage is the same typed transient failure as a
/// Home one; a rejected key or a contract violation keeps its own reason.
#[rstest]
#[case(
	Error::RemoteSemantic(crate::semantic::remote::Failure::Unavailable),
	crate::semantic::remote::Failure::Unavailable
)]
#[case(
	Error::RemoteSemantic(crate::semantic::remote::Failure::Configuration),
	crate::semantic::remote::Failure::Configuration
)]
#[case(
	Error::RemoteSemantic(crate::semantic::remote::Failure::ProviderContract),
	crate::semantic::remote::Failure::ProviderContract
)]
fn semantic_failures_keep_a_typed_reason_for_local_and_home_runs(
	#[case] error: Error,
	#[case] expected: crate::semantic::remote::Failure,
) {
	// Act
	let failure = crate::apps::execution::repositories::worker::classify_failure(&error);
	// Assert
	assert!(
		matches!(
			failure,
			aidash_application::recovery::ExecutionFailure::Semantic(reason) if reason == expected
		),
		"{failure:?}"
	);
}

/// `SemanticUnavailable` also reports invariant and allowance violations, such
/// as usage beyond a reservation; retrying cannot fix those, so it is not
/// classified as a transient semantic outage.
#[test]
fn semantic_invariant_failures_are_not_retried_as_outages() {
	// Act
	let failure =
		crate::apps::execution::repositories::worker::classify_failure(&Error::SemanticUnavailable);
	// Assert
	assert!(
		matches!(
			failure,
			aidash_application::recovery::ExecutionFailure::Other(_)
		),
		"{failure:?}"
	);
}

#[test]
fn tool_retry_survives_native_worker_error_conversion() {
	let native = Error::from(aidash_application::Error::ToolRetry {
		message: "not delivered".into(),
		count: 3,
		retry_after: Some(std::time::Duration::from_secs(11)),
	});
	let application = aidash_application::Error::from(native);
	let native = Error::from(application);
	assert!(
		matches!(crate::apps::execution::repositories::worker::classify_failure(&native),
		aidash_application::recovery::ExecutionFailure::ToolRetry { count: 3, retry_after: Some(delay), .. } if delay == std::time::Duration::from_secs(11))
	);
}

#[test]
fn a_refused_authority_change_says_so_and_when_to_try_again() {
	let response = Error::PolicyBusy { retry_after: 30 }.http_response();
	assert_eq!(response.status.as_u16(), 503);
	assert_eq!(response.headers["retry-after"], "30");
	assert_eq!(response.headers["x-aidash-policy-busy"], "30");
	assert!(
		!response
			.headers
			.contains_key("x-aidash-transaction-pending")
	);
	let body: Value = serde_json::from_slice(&response.body).unwrap();
	assert_eq!(body["error"], "POLICY_BUSY");
}

/// #728: a temporary failure used to end the Run for good.
#[rstest]
#[case("40001")]
#[case("40P01")]
#[case("55P03")]
#[case("57014")]
#[case("57P01")]
#[case("08006")]
fn a_temporary_database_error_defers_the_run(database_error: DatabaseError, #[case] code: &str) {
	// Arrange
	let error = Error::Framework(database_error.with_code(code).into());
	// Act
	let failure = crate::apps::execution::repositories::worker::classify_failure(&error);
	// Assert
	assert!(matches!(
		failure,
		aidash_application::recovery::ExecutionFailure::DatabaseUnavailable
	));
}

#[rstest]
fn a_permanent_database_error_still_fails_the_run(database_error: DatabaseError) {
	// Arrange
	let error = Error::Framework(database_error.with_code("23514").into());
	// Act
	let failure = crate::apps::execution::repositories::worker::classify_failure(&error);
	// Assert
	assert!(matches!(
		failure,
		aidash_application::recovery::ExecutionFailure::Other(_)
	));
}

/// #728: a real pool timeout, from a database nothing listens on, through the
/// same conversions a sweep and a Run step take.
#[rstest]
#[tokio::test]
async fn a_real_pool_timeout_is_a_temporary_database_error() {
	// Arrange
	let driver = sqlx::postgres::PgPoolOptions::new()
		.acquire_timeout(std::time::Duration::from_millis(100))
		.connect_lazy("postgres://nobody@127.0.0.1:1/nothing")
		.unwrap();
	let pool = crate::database::native::Pool::from(driver);
	let scope = aidash_application::tenancy::TenancyScope::Deployment(
		aidash_application::tenancy::DeploymentScopeReason::SemanticMaintenance,
	);
	// Act
	let error = pool.begin(&scope).await.err().unwrap();
	// Assert
	assert!(error.is_transient_database(), "{error}");
	assert!(matches!(
		crate::apps::execution::repositories::worker::classify_failure(&error),
		aidash_application::recovery::ExecutionFailure::DatabaseUnavailable
	));
	let application = aidash_application::Error::from(error);
	assert!(matches!(
		application,
		aidash_application::Error::TemporaryDatabase(_)
	));
	assert_eq!(
		Error::from(application).http_response().status.as_u16(),
		503
	);
}

/// Only a database that cannot be reached starts the post-outage hold on lease
/// recoveries; a lock wait, deadlock or statement timeout means it is up (#786).
#[rstest]
#[case::connection_lost(DatabaseErrorKind::Query, Some("08006"), true)]
#[case::shutting_down(DatabaseErrorKind::Query, Some("57P01"), true)]
#[case::client_deadline(DatabaseErrorKind::Timeout, None, true)]
#[case::refused(DatabaseErrorKind::Connection, None, true)]
#[case::deadlock(DatabaseErrorKind::Query, Some("40P01"), false)]
#[case::lock_wait(DatabaseErrorKind::Query, Some("55P03"), false)]
#[case::statement_timeout(DatabaseErrorKind::Query, Some("57014"), false)]
fn only_an_unreachable_database_holds_lease_recoveries(
	#[case] kind: DatabaseErrorKind,
	#[case] code: Option<&str>,
	#[case] unreachable: bool,
) {
	// Arrange
	let mut error = DatabaseError::new(kind, "private database diagnostic");
	if let Some(code) = code {
		error = error.with_code(code);
	}
	let error = Error::Framework(error.into());
	let outage = crate::apps::execution::repositories::store::run_state::Outage::default();
	// Act
	outage.note(&error);
	// Assert
	assert_eq!(error.is_database_unreachable(), unreachable);
	assert_eq!(outage.holds_recoveries(30), unreachable);
	assert!(!outage.holds_recoveries(0));
}
