use super::*;
use aidash_domain::{policy::Resource, provider::ToolCall};
use chrono::{TimeZone, Utc};
use rstest::rstest;

const PERSON: Uuid = Uuid::from_u128(5);

fn bundle(extra: Value) -> PolicyBundle {
	let mut policies = vec![
		json!({"id":"approve","effect":"allow","subjects":{"ids":["alice"]},"actions":["capability.approve"],"resources":{"kinds":["tool","outbound"]}}),
	];
	if !extra.is_null() {
		policies.push(extra);
	}
	serde_json::from_value(
		json!({"tenant":"tenant","subjects":{"alice":{"kind":"user"}},"policies":policies}),
	)
	.unwrap()
}

fn session(starter: Value) -> TestSession {
	TestSession {
		id: Uuid::from_u128(9),
		draft_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 1,
		status: "running".into(),
		scenario: starter,
		conversation: None,
		tool_calls: None,
		usage: json!({}),
		error: None,
		created_at: Utc.timestamp_opt(100, 0).unwrap(),
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		expires_at: Utc.timestamp_opt(200, 0).unwrap(),
		expired_at: None,
	}
}

fn started() -> TestSession {
	session(json!({"starter_identity":PERSON,"starter_subject":"alice"}))
}

fn approval(kind: &str, action: &'static str, requires_approval: bool) -> TestApproval {
	TestApproval {
		call: ToolCall {
			id: "call".into(),
			name: "tool".into(),
			arguments: json!({}),
		},
		resource: Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: "https://example.com".into(),
			attributes: json!({}),
		},
		action,
		requires_approval,
	}
}

fn alice(person: Option<Uuid>) -> Approver {
	Approver {
		subject: "alice".into(),
		person,
	}
}

#[test]
fn the_eligible_starter_may_answer() {
	assert_eq!(
		standing(
			&bundle(Value::Null),
			Some(&alice(Some(PERSON))),
			&started(),
			&approval("tool", "tool.call_unapproved", true),
			true
		),
		Standing::Askable
	);
}

#[rstest]
#[case::api_token(Some(alice(None)), started())]
#[case::another_person(Some(alice(Some(Uuid::from_u128(6)))), started())]
#[case::no_starter_recorded(Some(alice(Some(PERSON))), session(json!({})))]
#[case::starter_is_another_subject(Some(alice(Some(PERSON))), session(json!({"starter_identity":PERSON,"starter_subject":"bob"})))]
#[case::not_a_subject(None, started())]
fn nobody_but_the_starter_may_answer(#[case] approver: Option<Approver>, #[case] row: TestSession) {
	assert_eq!(
		standing(
			&bundle(Value::Null),
			approver.as_ref(),
			&row,
			&approval("tool", "tool.call_unapproved", true),
			true
		),
		Standing::Refused
	);
}

#[test]
fn a_call_the_policy_allows_needs_no_starter() {
	let allow = json!({"id":"call","effect":"allow","subjects":{"ids":["alice"]},"actions":["tool.call_unapproved"],"resources":{"kinds":["tool"]}});
	assert_eq!(
		standing(
			&bundle(allow),
			Some(&alice(None)),
			&session(json!({})),
			&approval("tool", "tool.call_unapproved", true),
			true
		),
		Standing::Allowed
	);
	// A read-only call needs no approval where no policy speaks of it.
	assert_eq!(
		standing(
			&bundle(Value::Null),
			Some(&alice(None)),
			&session(json!({})),
			&approval("tool", "tool.call_unapproved", false),
			true
		),
		Standing::Allowed
	);
}

#[test]
fn a_denied_call_cannot_be_approved() {
	let deny = json!({"id":"call","effect":"deny","subjects":{"ids":["alice"]},"actions":["tool.call_unapproved"],"resources":{"kinds":["tool"]}});
	assert_eq!(
		standing(
			&bundle(deny),
			Some(&alice(Some(PERSON))),
			&started(),
			&approval("tool", "tool.call_unapproved", true),
			true
		),
		Standing::Refused
	);
}

#[test]
fn an_ineligible_starter_cannot_answer() {
	let deny = json!({"id":"no-approval","effect":"deny","subjects":{"ids":["alice"]},"actions":["capability.approve"],"resources":{"kinds":["tool"]}});
	assert_eq!(
		standing(
			&bundle(deny),
			Some(&alice(Some(PERSON))),
			&started(),
			&approval("tool", "tool.call_unapproved", true),
			true
		),
		Standing::Refused
	);
}

#[test]
fn only_known_actions_and_admitted_origins_are_asked() {
	let starter = alice(Some(PERSON));
	let check = |approval: &TestApproval, admitted| {
		standing(
			&bundle(Value::Null),
			Some(&starter),
			&started(),
			approval,
			admitted,
		)
	};
	assert_eq!(
		check(&approval("tool", "registry.read", true), true),
		Standing::Refused
	);
	assert_eq!(
		check(&approval("outbound", "network.get", true), false),
		Standing::Refused
	);
	assert_eq!(
		check(&approval("outbound", "network.get", true), true),
		Standing::Askable
	);
}

#[test]
fn a_stored_request_names_the_same_approval() {
	let original = approval("tool", "tool.call_unapproved", true);
	let pending = json!({"call":original.call,"resource":original.resource,"action":original.action,"state":"pending"});
	let read = requested(&pending).unwrap();
	assert_eq!(read.call, original.call);
	assert_eq!(read.action, original.action);
	assert_eq!(
		serde_json::to_value(read.resource).unwrap(),
		serde_json::to_value(original.resource).unwrap()
	);
	let mut forged = pending.clone();
	forged["action"] = json!("registry.read");
	assert!(requested(&forged).is_none());
}

#[test]
fn concealing_removes_the_starter_and_the_request() {
	let mut row = started();
	row.scenario["pending_approval"] = json!({"state":"pending"});
	row.scenario["mode"] = json!("real");
	conceal(&mut row);
	assert_eq!(row.scenario, json!({"mode":"real"}));
}
