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
	} = CheckStatus::of(&result)
	else {
		panic!("expected a finished Check");
	};
	assert_eq!(Some((passed, exit_code)), expected);
	assert_eq!(diagnostics, result["output"].as_str().unwrap());
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
fn unknown_outcomes_are_unavailable_never_failed(#[case] result: serde_json::Value) {
	assert!(matches!(
		CheckStatus::of(&result),
		CheckStatus::Unavailable(_)
	));
}
