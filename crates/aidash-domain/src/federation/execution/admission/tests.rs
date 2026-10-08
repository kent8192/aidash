use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn binding() -> (Record, ExecutionPrincipal, Description) {
	let task = crate::Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "Task".into(),
		description: String::new(),
		status: crate::TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "source-subject".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 7,
		created_at: Utc::now(),
	};
	let inspection = super::super::tests::inspection();
	let description = Description {
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
		id: Uuid::from_u128(5),
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
