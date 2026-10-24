use super::*;
use serde_json::json;
#[rstest::rstest]
#[case("active")]
#[case("retained")]
#[case("recoverable")]
#[case("deleted")]
fn idle_generation_can_be_deleted(#[case] state: &str) {
	let area = area(state);
	assert!(require_idle(&area, area.revision).is_ok());
}
#[rstest::rstest]
#[case("running")]
#[case("uncertain")]
#[case("cleaning")]
#[case("cleanup_failed")]
fn unresolved_generation_must_be_reconciled(#[case] state: &str) {
	assert!(
		matches!(require_idle(&area(state),2),Err(crate::Error::Conflict(code)) if code.starts_with("AREA_BUSY:"))
	);
}
#[rstest::rstest]
fn stale_revision_is_rejected_before_retention() {
	assert!(
		matches!(require_idle(&area("active"),1),Err(crate::Error::Conflict(code)) if code=="AREA_REVISION_CHANGED")
	);
}
#[rstest::rstest]
fn duplicate_retention_choices_are_rejected() {
	let id = Uuid::new_v4();
	let file = || FileChoice {
		area_id: id,
		expected_revision: 2,
		choice: Choice::Keep,
		confirmation_id: None,
	};
	assert!(
		matches!(choices(vec![file(),file()]),Err(crate::Error::Invalid(code)) if code=="DUPLICATE_FILE_CHOICE")
	);
}
fn area(state: &str) -> Area {
	Area {
		id: Uuid::new_v4(),
		tenant: "tenant".into(),
		home_node: "home".into(),
		workspace_id: Uuid::new_v4(),
		thread_id: Uuid::new_v4(),
		agent_id: "agent".into(),
		owner: "owner".into(),
		generation: 1,
		revision: 2,
		epoch: 1,
		state: state.into(),
		manifest: json!([]),
		constraints: json!([]),
		next_sequence: 1,
	}
}
