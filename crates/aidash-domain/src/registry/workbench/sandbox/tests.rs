use super::*;
use chrono::TimeZone;
use rstest::rstest;
use serde_json::json;

fn limits() -> TestLimits {
	TestLimits {
		tenant: "tenant".into(),
		max_input_bytes: 1024,
		max_output_tokens: 64,
		max_total_tokens: 1_000_000,
		max_steps: 1,
		max_duration_secs: 5,
		max_concurrent: 1,
		payload_days: 1,
		incident_evidence_days: 1,
	}
}

fn selected() -> TestInput {
	serde_json::from_value(json!({"expected_revision":5,"message":"message"})).unwrap()
}

fn draft() -> super::super::Draft {
	super::super::Draft {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "owner".into(),
		revision: 5,
		entry: json!({}),
		documents: json!([]),
		release_notes: String::new(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: Utc.timestamp_opt(10, 0).unwrap(),
	}
}

fn previous() -> TestSession {
	TestSession {
		id: Uuid::from_u128(9),
		draft_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 5,
		status: "completed".into(),
		scenario: json!({"mode":"simulated"}),
		conversation: Some(json!([{"role":"assistant","content":"completed response"}])),
		tool_calls: Some(json!([])),
		usage: json!({}),
		error: None,
		created_at: Utc.timestamp_opt(10, 0).unwrap(),
		updated_at: Utc.timestamp_opt(20, 0).unwrap(),
		expires_at: Utc.timestamp_opt(30, 0).unwrap(),
		expired_at: None,
	}
}

#[rstest]
#[case("max_input_bytes", 1024, 1_000_000)]
#[case("max_output_tokens", 64, 8192)]
#[case("max_total_tokens", 256, 1_000_000)]
#[case("max_steps", 1, 64)]
#[case("max_duration_secs", 5, 900)]
#[case("max_concurrent", 1, 32)]
#[case("payload_days", 1, 365)]
#[case("incident_evidence_days", 1, 3650)]
fn execution_and_retention_limits_keep_the_original_inclusive_bounds(
	#[case] field: &str,
	#[case] lower: i32,
	#[case] upper: i32,
) {
	for (value, allowed) in [
		(lower - 1, false),
		(lower, true),
		(upper, true),
		(upper + 1, false),
	] {
		let mut wire = serde_json::to_value(limits()).unwrap();
		wire[field] = json!(value);
		let changed = serde_json::from_value(wire).unwrap();
		assert_eq!(
			validate_limits(&changed).is_ok(),
			allowed,
			"{field}={value}"
		);
	}
}

#[rstest]
fn output_budget_cannot_exceed_total_budget_even_when_each_field_is_in_range() {
	let mut value = limits();
	value.max_output_tokens = 1024;
	value.max_total_tokens = 256;
	assert_eq!(
		validate_limits(&value),
		Err(crate::Error::Invalid(
			"test limits are outside the supported ranges".into()
		))
	);
}

#[rstest]
#[case("message")]
#[case("mode")]
#[case("profile")]
#[case("fixtures")]
fn sandbox_admission_preserves_request_boundaries(#[case] boundary: &str) {
	let mut input = selected();
	match boundary {
		"message" => input.message = " ".into(),
		"mode" => input.mode = "production".into(),
		"profile" => input.profile_id = Some("real-profile".into()),
		_ => {
			for i in 0..65 {
				input.fixtures.insert(
					i.to_string(),
					Fixture {
						status: FixtureStatus::Success,
						response: json!({}),
					},
				);
			}
		}
	}
	assert!(validate_request(&input).is_err());
}

#[rstest]
fn default_simulation_and_typed_fixture_contracts_remain_unchanged() {
	let mut input = selected();
	assert_eq!(input.mode, "simulated");
	validate_request(&input).unwrap();
	for i in 0..64 {
		input.fixtures.insert(
			i.to_string(),
			Fixture {
				status: FixtureStatus::Denied,
				response: json!({"reason":"fixture"}),
			},
		);
	}
	validate_request(&input).unwrap();
	assert!(
		serde_json::from_value::<TestInput>(
			json!({"expected_revision":5,"message":"message","unknown":true})
		)
		.is_err()
	);
	assert_eq!(
		serde_json::to_value(FixtureStatus::Timeout).unwrap(),
		"timeout"
	);
}

#[rstest]
#[case("draft")]
#[case("revision")]
#[case("status")]
#[case("expiry")]
#[case("mode")]
#[case("profile")]
#[case("profile_revision")]
fn conversation_continuation_is_fenced_to_the_exact_completed_session(#[case] mismatch: &str) {
	let mut prior = previous();
	match mismatch {
		"draft" => prior.draft_id = Uuid::from_u128(2),
		"revision" => prior.revision = 4,
		"status" => prior.status = "outcome_unknown".into(),
		"expiry" => prior.expired_at = Some(Utc.timestamp_opt(40, 0).unwrap()),
		"mode" => prior.scenario["mode"] = json!("real"),
		"profile" => prior.scenario["profile_id"] = json!("profile"),
		_ => prior.scenario["profile_revision"] = json!(9),
	}
	assert_eq!(
		continued_conversation(prior, &draft(), &selected(), None),
		Err(crate::Error::Conflict(
			"previous test context is stale or unavailable; reset the conversation".into()
		))
	);
}

#[rstest]
fn completed_conversation_is_returned_exactly_and_missing_payload_is_rejected() {
	assert_eq!(
		continued_conversation(previous(), &draft(), &selected(), None).unwrap(),
		vec![json!({"role":"assistant","content":"completed response"})]
	);
	let mut prior = previous();
	prior.conversation = None;
	assert_eq!(
		continued_conversation(prior, &draft(), &selected(), None),
		Err(crate::Error::Conflict(
			"previous test conversation is unavailable".into()
		))
	);
}

#[rstest]
#[case(None, false)]
#[case(Some(json!([])), false)]
#[case(Some(json!({"outcome":"outcome_unknown"})), false)]
#[case(Some(json!([{"outcome":"denied"}])), false)]
#[case(Some(json!([{"outcome":"outcome_unknown"},{"outcome":"real"}])), true)]
fn recovery_detects_any_durable_unknown_outcome(
	#[case] calls: Option<Value>,
	#[case] unknown: bool,
) {
	assert_eq!(has_unknown_call(&calls), unknown);
}
