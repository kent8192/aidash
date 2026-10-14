use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[case::passed(json!({"status":"completed","termination_confirmed":true,"exit_code":0,"output":"ok"}), Some((true, Some(0))))]
#[case::nonzero(json!({"status":"completed","termination_confirmed":true,"exit_code":1,"output":"no"}), Some((false, Some(1))))]
#[case::failed(json!({"status":"failed","termination_confirmed":true,"exit_code":101,"output":"FAILED"}), Some((false, Some(101))))]
#[case::killed(json!({"status":"failed","termination_confirmed":true,"exit_code":null,"output":""}), Some((false, None)))]
fn finished_checks_pass_only_on_a_confirmed_zero_exit(
	#[case] result: serde_json::Value,
	#[case] expected: Option<(bool, Option<i64>)>,
) {
	let CheckStatus::Finished {
		passed,
		exit_code,
		diagnostics,
		runtime_seconds,
	} = CheckStatus::of(&result)
	else {
		panic!("expected a finished Check");
	};
	assert_eq!(Some((passed, exit_code)), expected);
	assert_eq!(diagnostics, result["output"].as_str().unwrap());
	assert_eq!(runtime_seconds, None);
}

/// A silent nonzero exit stores the runner's execution log as its error, which
/// the projection wraps in a structured code. It is the command's own failure
/// to repair, and the log is its diagnostics.
#[rstest]
#[case::conflict(json!({"code":"CONFLICT","message":"error: no such file","retryable":false}))]
#[case::log_word(json!({"code":"FAILED","message":"FAILED: 2 tests","retryable":false}))]
fn a_silent_failure_is_finished_with_its_log(#[case] error: serde_json::Value) {
	let result = json!({"status":"failed","termination_confirmed":true,"exit_code":1,
		"output":"","error":error,"runtime_seconds":1});
	let CheckStatus::Finished {
		passed,
		exit_code,
		diagnostics,
		runtime_seconds,
	} = CheckStatus::of(&result)
	else {
		panic!("expected a finished Check");
	};
	assert_eq!((passed, exit_code), (false, Some(1)));
	assert_eq!(diagnostics, error["message"].as_str().unwrap());
	assert_eq!(runtime_seconds, Some(1));
}

#[rstest]
#[case::submitted(json!({"status":"submitted"}))]
#[case::running(json!({"status":"running","output":"compiling"}))]
// A terminal state the runner has not confirmed stopped is still running.
#[case::unconfirmed(json!({"status":"completed","termination_confirmed":false,"exit_code":0}))]
fn unconfirmed_checks_are_still_running(#[case] result: serde_json::Value) {
	assert!(matches!(CheckStatus::of(&result), CheckStatus::Running));
}

#[rstest]
#[case::cancelled(json!({"status":"cancelled","termination_confirmed":true}))]
#[case::uncertain(json!({"status":"uncertain","error":"runner lost a previously accepted operation"}))]
#[case::withdrawn(json!({"status":"withdrawn"}))]
#[case::sandbox_lost(json!({"status":"failed","termination_confirmed":true,"exit_code":137,"error":{"code":"SANDBOX_LOST","message":"Sandbox stopped"}}))]
#[case::quota(json!({"status":"failed","termination_confirmed":true,"exit_code":null,"error":{"code":"WORKING_QUOTA_EXCEEDED","message":"Mounted files exceed the configured quota"}}))]
fn unknown_outcomes_are_unavailable_never_failed(#[case] result: serde_json::Value) {
	assert!(matches!(
		CheckStatus::of(&result),
		CheckStatus::Unavailable(_)
	));
}
