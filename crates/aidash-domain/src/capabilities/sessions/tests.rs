use super::*;
use rstest::rstest;
#[rstest]
#[case("active", true)]
#[case("running", true)]
#[case("deleted", false)]
#[case("recoverable", false)]
#[case("uncertain", false)]
#[case("cleaning", false)]
#[case("cleanup_failed", false)]
#[case("retained", false)]
fn retained_context_cannot_be_reused(#[case] state: &str, #[case] expected: bool) {
	assert_eq!(reusable_context(state), expected);
}
#[rstest]
#[case(vec!["alice","parent","agent"],"alice","agent",true)]
#[case(vec!["mallory","agent"],"alice","agent",false)]
#[case(vec!["alice","other"],"alice","agent",false)]
#[case(vec![],"alice","agent",false)]
fn disclosure_requires_both_delegation_endpoints(
	#[case] subjects: Vec<&str>,
	#[case] owner: &str,
	#[case] agent: &str,
	#[case] expected: bool,
) {
	assert_eq!(
		received_scope_matches(
			&subjects.into_iter().map(str::to_owned).collect::<Vec<_>>(),
			owner,
			agent
		),
		expected
	);
}

// Receiving Areas (ADR 0139).
use serde_json::json;

fn area(constraints: Value) -> Area {
	Area {
		id: Uuid::nil(),
		tenant: "tenant".into(),
		home_node: "aidash://home/node".into(),
		workspace_id: Uuid::nil(),
		thread_id: Uuid::nil(),
		agent_id: "agent".into(),
		owner: "alice".into(),
		generation: 0,
		revision: 0,
		epoch: 0,
		state: "active".into(),
		manifest: json!([]),
		constraints,
		next_sequence: 1,
	}
}

#[test]
fn only_a_receiving_area_names_its_home() {
	assert!(!area(json!([])).is_remote_home());
	assert!(!area(json!([{"kind":"reference_bound","id":Uuid::nil()}])).is_remote_home());
	assert!(area(json!([remote_home_constraint("aidash://home/node")])).is_remote_home());
	assert!(!area(json!("malformed")).is_remote_home());
}
