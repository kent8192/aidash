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
#[case("40001", 500, None, true)]
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
