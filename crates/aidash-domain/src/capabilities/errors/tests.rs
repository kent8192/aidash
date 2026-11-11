use super::*;
use rstest::rstest;

#[rstest]
#[case::null(json!(null))]
#[case::empty(json!(""))]
#[case::blank(json!(" \n"))]
fn a_stored_error_with_nothing_to_say_is_no_error(#[case] value: Value) {
	assert!(CapabilityError::stored(&value).is_none());
}

#[rstest]
#[case::runner_text("runner lost a previously accepted operation", EXECUTION_UNVERIFIED)]
#[case::its_own_code("AREA_BUSY: a command is running", "AREA_BUSY")]
fn plain_stored_text_is_never_called_a_conflict(#[case] text: &str, #[case] code: &str) {
	let error = CapabilityError::stored(&json!(text)).unwrap();
	assert_eq!(error.code, code);
	assert_eq!(error.message, text);
	assert!(!error.retryable);
}

#[rstest]
fn a_stale_revision_stored_as_text_keeps_the_revision_to_retry_with() {
	let error = CapabilityError::stored(&json!(area_revision_changed(3))).unwrap();
	assert_eq!(error.code, "AREA_REVISION_CHANGED");
	assert_eq!(error.details, Some(json!({"current_revision": 3})));
}

#[rstest]
fn a_refused_request_still_reads_as_a_conflict() {
	assert_eq!(CapabilityError::message(409, "taken").code, "CONFLICT");
}

#[rstest]
fn a_typed_runner_error_keeps_its_code() {
	let error = CapabilityError::stored(&json!({"code":"COMMAND_FAILED","message":"log"})).unwrap();
	assert_eq!(
		(error.code.as_str(), error.message.as_str()),
		("COMMAND_FAILED", "log")
	);
}

#[rstest]
fn a_timed_out_command_says_what_was_saved_and_what_to_do_next() {
	let error = CapabilityError::command_timed_out(&TimedOut {
		limit_seconds: 120,
		maximum_seconds: 600,
		revision: 22,
		python: false,
		install: false,
	});
	assert_eq!(error.code, COMMAND_TIMED_OUT);
	assert!(!error.retryable);
	assert_eq!(
		error.details,
		Some(json!({"limit_seconds": 120, "revision": 22}))
	);
	for part in [
		"120 s",
		"/work",
		"revision 22",
		"/tmp",
		"again",
		"timeout_seconds",
		"600",
	] {
		assert!(error.message.contains(part), "{part} in {}", error.message);
	}
}

#[rstest]
fn a_command_at_the_maximum_is_not_told_to_raise_its_limit() {
	let error = CapabilityError::command_timed_out(&TimedOut {
		limit_seconds: 600,
		maximum_seconds: 600,
		revision: 22,
		python: false,
		install: false,
	});
	assert!(
		!error.message.contains("timeout_seconds"),
		"{}",
		error.message
	);
	assert!(
		error.message.contains("shorter commands"),
		"{}",
		error.message
	);
}

#[rstest]
fn a_timed_out_python_cell_says_its_session_was_reset() {
	let error = CapabilityError::command_timed_out(&TimedOut {
		limit_seconds: 60,
		maximum_seconds: 600,
		revision: 4,
		python: true,
		install: false,
	});
	assert_eq!(error.code, COMMAND_TIMED_OUT);
	for part in [
		"60 s",
		"session was reset",
		"revision 4",
		"variables",
		"shorter cells",
	] {
		assert!(error.message.contains(part), "{part} in {}", error.message);
	}
}

#[rstest]
fn a_timed_out_error_survives_being_stored() {
	let error = CapabilityError::command_timed_out(&TimedOut {
		limit_seconds: 60,
		maximum_seconds: 600,
		revision: 4,
		python: false,
		install: false,
	});
	let stored = CapabilityError::stored(&serde_json::to_value(&error).unwrap()).unwrap();
	assert_eq!(stored.code, COMMAND_TIMED_OUT);
	assert_eq!(stored.details, error.details);
}

#[rstest]
#[case::no_full_stop("nothing was saved")]
#[case::full_stop("nothing was saved.")]
fn a_refusal_past_the_limit_reads_as_two_sentences(#[case] message: &str) {
	assert_eq!(
		also_timed_out(message, 5),
		"nothing was saved. The command had also reached its 5 s time limit."
	);
}
