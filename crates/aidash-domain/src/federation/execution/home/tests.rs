use super::*;
use rstest::rstest;
use serde_json::json;
#[rstest]
#[case::admission(2,3,json!({"task":"original"}),false)]
#[case::task(1,4,json!({"task":"original"}),false)]
#[case::snapshot(1,3,json!({"task":"changed"}),false)]
#[case::matching(1,3,json!({"task":"original"}),true)]
fn home_binding_requires_exact_admission_task_and_initial_snapshot(
	#[case] admission: u128,
	#[case] task: u128,
	#[case] snapshot: Value,
	#[case] expected: bool,
) {
	let bound = HomeBinding {
		grant_id: Uuid::from_u128(5),
		admission_id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(3),
		task_revision: 7,
		initial_task: json!({"task":"original"}),
	};
	assert_eq!(
		bound.matches(Uuid::from_u128(admission), Uuid::from_u128(task), &snapshot),
		expected
	);
}
