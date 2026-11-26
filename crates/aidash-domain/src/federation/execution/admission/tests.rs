use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn binding() -> (Record, ExecutionPrincipal, Description) {
	let task = crate::Task {
		id: Uuid::from_u128(1),
		thread_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: String::new(),
		status: crate::TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "source-subject".into(),
		dependencies: vec![],
		parent_id: None,
		task_model: None,
		started_by_duty: None,
		revision: 7,
		created_at: Utc::now(),
	};
	let inspection = super::super::tests::inspection();
	let description = Description {
		authorization_details: Vec::new(),
		grant_id: Uuid::from_u128(3),
		source_node: "aidash://home".into(),
		target_node: "aidash://receiver".into(),
		source_tenant: "source-tenant".into(),
		source_subject: "source-subject".into(),
		task,
		inspection,
		expires_at: Utc::now(),
		semantic: crate::semantic::remote::Binding::Disabled {},
	};
	let identity = ExecutionPrincipal {
		tenant: "receiver-tenant".into(),
		subject: "mapped".into(),
		credential_id: Uuid::from_u128(4),
	};
	let record = Record {
		cancelled: false,
		id: Uuid::from_u128(5),
		node: crate::nodes::NodeName::parse("calm-heron-ab12cd34").unwrap(),
		source_node: description.source_node.clone(),
		grant_id: description.grant_id,
		task_id: description.task.id,
		tenant: identity.tenant.clone(),
		credential_id: identity.credential_id,
		subject_chain: vec!["mapped".into(), "agent".into()],
		description: serde_json::to_value(&description).unwrap(),
	};
	(record, identity, description)
}

#[rstest]
fn the_request_rides_the_description_only_when_there_is_one(
	binding: (Record, ExecutionPrincipal, Description),
) {
	use crate::federation::tool_lists::{FederationToolRequest, ToolAuthorizationDetail};
	let (_, _, mut description) = binding;
	assert!(
		serde_json::to_value(&description)
			.unwrap()
			.get("authorization_details")
			.is_none()
	);
	description.authorization_details = vec![ToolAuthorizationDetail::new(
		&FederationToolRequest::defaults(),
	)];
	let wire = serde_json::to_value(&description).unwrap();
	assert_eq!(
		wire["authorization_details"][0]["type"],
		"aidash.federation.tools"
	);
	let back: Description = serde_json::from_value(wire).unwrap();
	assert_eq!(
		back.authorization_details,
		description.authorization_details
	);
}

#[rstest]
fn identical_admission_retains_the_original_binding(
	binding: (Record, ExecutionPrincipal, Description),
) {
	let (record, identity, description) = binding;
	assert!(
		record
			.matches(&identity, &["mapped".into(), "agent".into()], &description)
			.unwrap()
	);
	assert_eq!(record.view(&description).id, record.id);
	assert_eq!(record.view(&description).expires_at, description.expires_at);
}

/// The Home renews the grant and its tool Request changes between describes; neither
/// ends the admission (ADR 0139).
#[rstest]
fn a_renewed_grant_or_a_changed_tool_request_keeps_the_admission(
	binding: (Record, ExecutionPrincipal, Description),
) {
	let (record, identity, mut description) = binding;
	description.expires_at += chrono::Duration::hours(1);
	description.authorization_details =
		vec![crate::federation::tool_lists::ToolAuthorizationDetail::new(
			&crate::federation::tool_lists::FederationToolRequest::defaults(),
		)];
	assert!(
		record
			.matches(&identity, &["mapped".into(), "agent".into()], &description)
			.unwrap()
	);
}

/// The admission keeps the Request the Home last described, widened or narrowed, so checks that
/// hold no Home connection read it (ADR 0139, decision 12); nothing else it pinned moves.
#[rstest]
fn the_admission_keeps_the_request_the_home_last_described(
	binding: (Record, ExecutionPrincipal, Description),
) {
	use crate::federation::tool_lists::{
		FederationToolRequest, HostPattern, ToolAuthorizationDetail, ToolList, ToolPattern,
	};
	let (mut record, identity, description) = binding;
	let chain = ["mapped".to_owned(), "agent".to_owned()];
	let request = |tools: &[&str], hosts: &[&str]| {
		vec![ToolAuthorizationDetail::new(&FederationToolRequest {
			tools: ToolList::new(tools.iter().map(|tool| ToolPattern::new(*tool).unwrap()))
				.unwrap(),
			hosts: hosts
				.iter()
				.map(|host| HostPattern::new(*host).unwrap())
				.collect(),
		})]
	};
	let described = |details| {
		let mut fresh = description.clone();
		fresh.authorization_details = details;
		// A renewal moves the expiry the admission does not take.
		fresh.expires_at += chrono::Duration::hours(1);
		fresh
	};
	let stored = |record: &Record| {
		serde_json::from_value::<Description>(record.description.clone()).unwrap()
	};
	// The Home answers an ask by adding `shell` and a host.
	let widened = described(request(&["shell", "thread_*"], &["example.com"]));
	record.description = record.with_tool_request(&widened).unwrap().unwrap();
	assert_eq!(
		stored(&record).authorization_details,
		widened.authorization_details
	);
	assert_eq!(stored(&record).expires_at, description.expires_at);
	assert!(record.matches(&identity, &chain, &widened).unwrap());
	// Described again unchanged: nothing to write.
	assert!(record.with_tool_request(&widened).unwrap().is_none());
	// The Home narrows its tools and hosts.
	let narrowed = described(request(&["thread_*"], &[]));
	record.description = record.with_tool_request(&narrowed).unwrap().unwrap();
	assert_eq!(
		stored(&record).authorization_details,
		narrowed.authorization_details
	);
	// A Request withdrawn altogether is stored as a Description without one.
	let none = described(Vec::new());
	record.description = record.with_tool_request(&none).unwrap().unwrap();
	assert!(record.description.get("authorization_details").is_none());
	assert!(record.with_tool_request(&description).unwrap().is_none());
	assert!(record.matches(&identity, &chain, &description).unwrap());
}

#[rstest]
#[case(0)]
#[case(1)]
#[case(2)]
#[case(3)]
#[case(4)]
#[case(5)]
#[case(6)]
fn a_replayed_id_cannot_change_authority(
	binding: (Record, ExecutionPrincipal, Description),
	#[case] field: usize,
) {
	let (mut record, identity, description) = binding;
	match field {
		0 => record.source_node = "aidash://other".into(),
		1 => record.grant_id = Uuid::from_u128(99),
		2 => record.task_id = Uuid::from_u128(99),
		3 => record.tenant = "other".into(),
		4 => record.credential_id = Uuid::from_u128(99),
		5 => record.subject_chain.reverse(),
		6 => record.description["inspection"]["authority_digest"] = json!("changed"),
		_ => panic!("unknown binding mutation"),
	}
	assert!(
		!record
			.matches(&identity, &["mapped".into(), "agent".into()], &description)
			.unwrap()
	);
}

#[rstest]
#[case(RemoteExecutionControl::Pause, "pause")]
#[case(RemoteExecutionControl::Resume, "resume")]
#[case(RemoteExecutionControl::Cancel, "cancel")]
fn peer_controls_keep_the_wire_spelling(
	#[case] action: RemoteExecutionControl,
	#[case] expected: &str,
) {
	assert_eq!(serde_json::to_value(action).unwrap(), json!(expected));
}

#[rstest]
fn peer_phase_and_inactive_control_keep_the_protocol_spelling() {
	assert_eq!(
		serde_json::to_value(RemoteExecutionPhase::from(crate::RunPhase::ToolCall)).unwrap(),
		json!("TOOL_CALL")
	);
	assert_eq!(
		serde_json::to_value(RemoteExecutionControlState::Inactive).unwrap(),
		json!("INACTIVE")
	);
}

#[rstest]
fn activation_carries_a_context_pause_reason_only_when_present() {
	let mut activation = Activation {
		grant_id: Uuid::from_u128(1),
		admission_id: Uuid::from_u128(2),
		run_id: Uuid::from_u128(2),
		phase: RemoteExecutionPhase::Thinking,
		control: RemoteExecutionControlState::Paused,
		error: None,
		semantic_reason: None,
		context_reason: None,
		declinable_call: None,
	};
	// Peers without the field keep their exact wire shape.
	let legacy = serde_json::to_value(&activation).unwrap();
	assert!(legacy.get("context_reason").is_none());
	assert!(
		serde_json::from_value::<Activation>(legacy)
			.unwrap()
			.context_reason
			.is_none()
	);
	activation.context_reason = Some(crate::context::recovery::Failure::SummaryInvalid);
	let paused = serde_json::to_value(&activation).unwrap();
	assert_eq!(paused["context_reason"], json!("summary_invalid"));
	assert_eq!(
		serde_json::from_value::<Activation>(paused)
			.unwrap()
			.context_reason,
		Some(crate::context::recovery::Failure::SummaryInvalid)
	);
}

#[rstest]
fn activation_preserves_a_specific_budget_cause_without_stranding_older_peers() {
	let mut wire = json!({
		"grant_id":Uuid::from_u128(1),"admission_id":Uuid::from_u128(2),
		"run_id":Uuid::from_u128(2),"phase":"THINKING","control":"PAUSED",
		"error":null,"semantic_reason":"durable_memory_budget"
	});
	let activation: Activation = serde_json::from_value(wire.clone()).unwrap();
	wire = serde_json::to_value(activation).unwrap();
	assert_eq!(wire["semantic_reason"], "context_budget");
	assert_eq!(wire["semantic_cause"], "durable_memory_budget");
	let schema = serde_json::to_value(schemars::schema_for!(Activation)).unwrap();
	assert!(jsonschema::is_valid(&schema, &wire));
	assert_eq!(
		serde_json::from_value::<Activation>(wire.clone())
			.unwrap()
			.semantic_reason,
		Some(Failure::DurableMemoryBudget)
	);
	// Older peers consume the established reason and ignore the added field.
	#[derive(Deserialize)]
	struct OlderActivation {
		semantic_reason: Failure,
	}
	assert_eq!(
		serde_json::from_value::<OlderActivation>(wire.clone())
			.unwrap()
			.semantic_reason,
		Failure::ContextBudget
	);
	// A future cause still leaves the compatible, follow-up-only reason intact.
	wire["semantic_cause"] = json!("future_budget_cause");
	assert_eq!(
		serde_json::from_value::<Activation>(wire.clone())
			.unwrap()
			.semantic_reason,
		Some(Failure::ContextBudget)
	);
	wire.as_object_mut().unwrap().remove("semantic_cause");
	assert_eq!(
		serde_json::from_value::<Activation>(wire)
			.unwrap()
			.semantic_reason,
		Some(Failure::ContextBudget)
	);
}

#[rstest]
fn cancelled_admission_keeps_original_binding_but_rejects_fresh_authority(
	binding: (Record, ExecutionPrincipal, Description),
) {
	let (mut record, identity, description) = binding;
	record.cancelled = true;
	assert!(
		!record
			.matches(&identity, &record.subject_chain, &description)
			.unwrap()
	);
	assert_eq!(record.view(&description).id, record.id);
}

#[rstest::rstest]
#[case::thinking(crate::RunPhase::Thinking, 3, true)]
#[case::waiting_past_its_lifetime(crate::RunPhase::Waiting, 7, true)]
#[case::never_started(crate::RunPhase::Ready, 0, false)]
fn a_receiver_serves_its_home_for_a_started_run_past_the_lifetime(
	#[case] phase: crate::RunPhase,
	#[case] step: i32,
	#[case] served: bool,
) {
	let now = chrono::Utc::now();
	let lapsed = now - chrono::Duration::seconds(1);
	assert_eq!(
		serves_home(phase, crate::RunControl::Active, step, lapsed, now),
		served
	);
	// Within its lifetime even a Run that never started is served.
	assert!(serves_home(
		crate::RunPhase::Ready,
		crate::RunControl::Active,
		0,
		now + chrono::Duration::hours(1),
		now
	));
}

#[rstest::rstest]
#[case::completed(crate::RunPhase::Completed, crate::RunControl::Active)]
#[case::failed(crate::RunPhase::Failed, crate::RunControl::Active)]
#[case::cancelled_phase(crate::RunPhase::Cancelled, crate::RunControl::Active)]
#[case::cancelled_control(crate::RunPhase::Waiting, crate::RunControl::Cancelled)]
fn a_receiver_never_serves_its_home_for_an_ended_run(
	#[case] phase: crate::RunPhase,
	#[case] control: crate::RunControl,
) {
	let now = chrono::Utc::now();
	assert!(!serves_home(
		phase,
		control,
		5,
		now + chrono::Duration::hours(1),
		now
	));
}
