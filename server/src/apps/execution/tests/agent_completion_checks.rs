//! How the Completion Gate (#112) reads a Check start or poll on this node.
use super::{check_polled, check_started};
use crate::Error;
use aidash_application::ports::execution::completion::{CheckStart, CheckStatus};
use serde_json::json;
use uuid::Uuid;

#[rstest::rstest]
fn a_started_check_keeps_its_operation_and_timeout() {
	let id = Uuid::from_u128(7);
	let start = check_started(Ok(json!({"operation_id":id,"status":"prepared"})), 42).unwrap();
	assert!(matches!(
		start,
		CheckStart::Started {
			operation_id,
			timeout_seconds: 42,
		} if operation_id == id
	));
}

#[rstest::rstest]
fn a_start_without_an_operation_is_invalid() {
	assert!(matches!(
		check_started(Ok(json!({"status":"prepared"})), 1),
		Err(aidash_application::Error::Invalid(_))
	));
}

/// The content moved after the gate read it, or the same attempt was started
/// at another revision before a process cut: the gate reads it again.
#[rstest::rstest]
#[case::revision("AREA_REVISION_CHANGED")]
#[case::busy("AREA_BUSY: another operation is running")]
#[case::replayed_attempt("IDEMPOTENCY_CONFLICT")]
fn moved_content_makes_the_start_stale(#[case] reason: &str) {
	assert!(matches!(
		check_started(Err(Error::Conflict(reason.into())), 1).unwrap(),
		CheckStart::Stale
	));
}

#[rstest::rstest]
#[case::forbidden(Error::Forbidden)]
#[case::unauthorized(Error::Unauthorized)]
fn refused_authority_denies_the_check(#[case] error: Error) {
	assert!(matches!(
		check_started(Err(error), 1).unwrap(),
		CheckStart::Denied(_)
	));
}

/// The shell Binding's narrowing is part of the Check's authority.
#[rstest::rstest]
#[case::scope("tool argument command exceeds Binding scope")]
#[case::limit("tool argument timeout_seconds exceeds Binding limit")]
fn a_narrowing_refusal_denies_the_check_with_its_reason(#[case] reason: &str) {
	let CheckStart::Denied(kept) = check_started(Err(Error::Invalid(reason.into())), 1).unwrap()
	else {
		panic!("expected a denied Check");
	};
	assert_eq!(kept, reason);
}

#[rstest::rstest]
#[case::no_runner(Error::Conflict(
	"RUNTIME_UNAVAILABLE: no isolated runner configured".into()
))]
#[case::unbound(Error::Invalid("Run has no bound operation: shell".into()))]
#[case::no_area(Error::NotFound("working area unavailable".into()))]
fn other_refusals_keep_their_reason_as_unavailable(#[case] error: Error) {
	let reason = error.to_string();
	let CheckStart::Unavailable(kept) = check_started(Err(error), 1).unwrap() else {
		panic!("expected an unavailable Check");
	};
	assert_eq!(kept, reason);
}

#[rstest::rstest]
fn other_start_failures_stop_the_step() {
	assert!(check_started(Err(Error::TransactionPending), 1).is_err());
}

#[rstest::rstest]
fn a_polled_check_is_read_by_its_operation_status() {
	assert!(matches!(
		check_polled(Ok(json!({"status":"running"}))).unwrap(),
		CheckStatus::Running
	));
	let CheckStatus::Finished {
		passed, exit_code, ..
	} = check_polled(Ok(
		json!({"status":"completed","termination_confirmed":true,"exit_code":0,"output":"ok"}),
	))
	.unwrap()
	else {
		panic!("expected a finished Check");
	};
	assert!(passed);
	assert_eq!(exit_code, Some(0));
}

#[rstest::rstest]
fn a_lost_operation_is_unavailable_but_revoked_authority_stops_the_step() {
	assert!(matches!(
		check_polled(Err(Error::NotFound("operation unavailable".into()))).unwrap(),
		CheckStatus::Unavailable(reason) if reason == "operation unavailable"
	));
	assert!(check_polled(Err(Error::Forbidden)).is_err());
}
