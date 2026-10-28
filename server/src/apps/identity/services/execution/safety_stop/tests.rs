use super::SafetyStopInput;
use crate::Error;
use crate::apps::identity::services::execution::management::ManagementRequest;
use rstest::rstest;
use serde_json::{Value, json};

fn request(body: Value) -> ManagementRequest {
	serde_json::from_value(body).expect("the request body always deserializes")
}

fn administrator() -> Value {
	json!({
		"tenant": "acme",
		"action": "pause",
		"expected_control_revision": 3,
		"reason": "runaway spend",
	})
}

#[rstest]
fn an_administrator_body_is_read_as_a_safety_stop() {
	let SafetyStopInput {
		tenant,
		expected_control_revision,
		reason,
		..
	} = request(administrator()).for_administrator().unwrap();
	assert_eq!(
		(tenant.as_str(), expected_control_revision, reason.as_str()),
		("acme", 3, "runaway spend")
	);
}

#[rstest]
#[case(json!({"action": "pause"}))]
#[case(json!({"action": "cancel", "extra": 1}))]
#[case(json!([1, 2]))]
fn an_administrator_must_send_the_whole_command(#[case] body: Value) {
	assert!(matches!(
		request(body).for_administrator(),
		Err(Error::Invalid(_))
	));
}

#[rstest]
fn a_subject_body_keeps_its_contract() {
	assert!(request(json!({"action": "cancel"})).for_subject().is_ok());
}

#[rstest]
#[case(json!({"action": "pause", "reason": "x"}))]
#[case(json!({"action": "pause", "tenant": "acme"}))]
#[case(json!({"action": "pause", "expected_control_revision": 0}))]
fn a_subject_may_not_send_administrator_fields(#[case] body: Value) {
	assert!(matches!(
		request(body).for_subject(),
		Err(Error::Invalid(_))
	));
	assert!(matches!(
		request(administrator()).for_subject(),
		Err(Error::Invalid(_))
	));
}

#[rstest]
#[case(json!({"action": "resume"}))]
#[case(json!({"unknown": true}))]
fn a_malformed_subject_body_is_unchanged(#[case] body: Value) {
	assert!(matches!(
		request(body).for_subject(),
		Err(Error::Unprocessable(_))
	));
}

#[rstest]
fn the_projection_names_exactly_the_allowlisted_fields() {
	use super::RunControlFacts;
	use aidash_domain::{ControlSource, RunControl, RunPhase};
	let facts = RunControlFacts {
		id: uuid::Uuid::nil(),
		phase: RunPhase::Thinking,
		control: RunControl::Paused,
		control_revision: 4,
		control_source: ControlSource::DeploymentAdministrator,
	};
	let mut keys: Vec<_> = serde_json::to_value(facts)
		.unwrap()
		.as_object()
		.unwrap()
		.keys()
		.cloned()
		.collect();
	keys.sort();
	assert_eq!(
		keys,
		[
			"control",
			"control_revision",
			"control_source",
			"id",
			"phase"
		]
	);
}
