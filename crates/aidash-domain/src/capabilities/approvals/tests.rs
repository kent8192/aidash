use super::*;
use crate::policy::{Resource, SubjectKind};
use rstest::{fixture, rstest};

#[fixture]
fn bundle() -> PolicyBundle {
	serde_json::from_value(json!({"tenant":"tenant","subjects":{"user":{"kind":"user"},"operator":{"kind":"user","attributes":{"capability_approver":true}},"agent":{"kind":"agent","attributes":{"capability_approver":true}}},"policies":[{"id":"approve","effect":"allow","subjects":{"any":true},"actions":["capability.approve"],"resources":{"kinds":["outbound"]},"condition":{"op":"eq","left":{"source":"environment","path":"/transport"},"right":{"source":"literal","value":"api"}}}]})).unwrap()
}
fn evaluation(subject: &str) -> Evaluation {
	Evaluation {
		subject: subject.into(),
		action: "capability.approve".into(),
		resource: Resource {
			tenant: "tenant".into(),
			kind: "outbound".into(),
			id: "https://example.com".into(),
			attributes: json!({}),
		},
		environment: json!({"transport":"api"}),
	}
}

#[rstest]
#[case::requester("user", "user", true)]
#[case::designated("operator", "user", true)]
#[case::ordinary_other_user("user", "another", false)]
#[case::agent("agent", "user", false)]
#[case::unknown("unknown", "user", false)]
fn an_approver_must_be_an_enabled_authorized_human(
	bundle: PolicyBundle,
	#[case] subject: &str,
	#[case] requester: &str,
	#[case] eligible: bool,
) {
	assert_eq!(
		approver_eligible(&bundle, &evaluation(subject), requester),
		eligible
	);
}

#[rstest]
#[case::disabled("disabled")]
#[case::no_allow("no_allow")]
#[case::explicit_deny("deny")]
#[case::worker_transport("worker")]
fn designation_never_overrides_current_policy_or_subject_state(
	mut bundle: PolicyBundle,
	#[case] changed: &str,
) {
	let mut request = evaluation("operator");
	match changed {
		"disabled" => bundle.subjects.get_mut("operator").unwrap().enabled = false,
		"no_allow" => bundle.policies.clear(),
		"deny" => {
			let mut policy = bundle.policies[0].clone();
			policy.id = "deny".into();
			policy.effect = crate::policy::Effect::Deny;
			bundle.policies.push(policy);
		}
		"worker" => request.environment["transport"] = json!("worker"),
		_ => panic!("unknown fixture"),
	}
	assert!(!approver_eligible(&bundle, &request, "user"));
}

#[rstest]
fn a_nonhuman_cannot_approve_even_when_it_is_the_requester(mut bundle: PolicyBundle) {
	bundle.subjects.get_mut("user").unwrap().kind = SubjectKind::Service;
	assert!(!approver_eligible(&bundle, &evaluation("user"), "user"));
}

#[rstest]
#[case::ordinary(true, false, false, "approved")]
#[case::grant(false, true, false, "approved")]
#[case::approval(false, false, true, "pending")]
#[case::unapprovable(false, false, false, "blocked")]
fn requests_preserve_ordinary_grant_and_human_approval_precedence(
	#[case] direct: bool,
	#[case] grant: bool,
	#[case] designated: bool,
	#[case] state: &str,
) {
	assert_eq!(requested_state(direct, grant, designated), state);
}

#[rstest]
#[case::pending("pending", false, "approval_required", "pending")]
#[case::expired("pending", true, "blocked", "expired")]
#[case::denied("denied", false, "blocked", "denied")]
#[case::approved("approved", false, "running", "approved")]
#[case::attempted("attempted", true, "running", "attempted")]
#[case::completed("completed", true, "completed", "completed")]
fn projection_keeps_the_existing_status_and_expiry_contract(
	#[case] state: &str,
	#[case] expired: bool,
	#[case] status: &str,
	#[case] approval_state: &str,
) {
	let record = Record {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "user".into(),
		area_id: None,
		kind: "outbound".into(),
		state: state.into(),
		revision: 7,
		data: json!({"targets":["https://example.com"],"approver":"operator","attempted_at":"saved-timestamp"}),
		expires_at: None,
	};
	let value = projection(&record, expired);
	assert_eq!(value["status"], status);
	assert_eq!(value["approval_state"], approval_state);
	assert_eq!(value["approval_revision"], 7);
	assert_eq!(value["effects_may_have_occurred"], true);
	assert_eq!(value["targets"], record.data["targets"]);
}

#[rstest]
fn the_decision_defaults_and_unknown_fields_match_the_http_contract() {
	let key = Uuid::from_u128(1);
	let value = json!({"idempotency_key":key,"expected_revision":3});
	let decision: ApprovalDecision = serde_json::from_value(value.clone()).unwrap();
	assert!(matches!(decision.choice, ApprovalChoice::AllowOnce));
	let mut unexpected = value;
	unexpected["unexpected"] = json!(true);
	assert!(serde_json::from_value::<ApprovalDecision>(unexpected).is_err());
}
