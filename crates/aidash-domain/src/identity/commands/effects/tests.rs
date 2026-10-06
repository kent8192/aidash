use super::*;
use rstest::{fixture, rstest};
#[fixture]
fn task() -> Task {
	Task {
		id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		title: String::new(),
		description: String::new(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "parent".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 1,
		created_at: chrono::Utc::now(),
	}
}
#[rstest]
fn a_new_child_of_the_bound_task_is_eligible_for_grant_provenance_checks(task: Task) {
	let child = Task {
		id: Uuid::from_u128(5),
		parent_id: Some(task.id),
		created_by: "owner".into(),
		..task.clone()
	};
	assert!(child_eligible(&task, &child, "owner"));
}
#[rstest]
#[case::workspace("workspace")]
#[case::parent("parent")]
#[case::creator("creator")]
#[case::owner("owner")]
#[case::status("status")]
fn delegation_cannot_claim_unrelated_owned_or_started_children(task: Task, #[case] change: &str) {
	let mut child = Task {
		id: Uuid::from_u128(5),
		parent_id: Some(task.id),
		created_by: "owner".into(),
		..task.clone()
	};
	match change {
		"workspace" => child.workspace_id = Uuid::nil(),
		"parent" => child.parent_id = None,
		"creator" => child.created_by = "other".into(),
		"owner" => child.owner = Some("other".into()),
		"status" => child.status = TaskStatus::Claimed,
		_ => unreachable!(),
	}
	assert!(!child_eligible(&task, &child, "owner"));
}
#[rstest]
fn tool_audit_metadata_removes_arguments_results_and_unrelated_fields() {
	let run = Uuid::from_u128(1);
	let input = json!({"kind":"remote.tool.completed","data":{"run_id":run,"call":{"id":"call","name":"tool","arguments":{"protected":"input"}},"result":{"protected":"output"}}});
	assert_eq!(
		audit_event(&input, run).unwrap(),
		(
			"task.remote_tool_completed",
			json!({"call":{"id":"call","name":"tool"}})
		)
	);
}
#[rstest]
fn recovery_audits_keep_only_the_validated_phase_and_cause() {
	let run = Uuid::from_u128(1);
	let input = json!({"kind":"remote.run.recovered","data":{"run_id":run,"phase":"resume","cause":"expired worker lease","protected":"discard"}});
	assert_eq!(
		audit_event(&input, run).unwrap(),
		(
			"task.remote_run_recovered",
			json!({"phase":"resume","cause":"expired worker lease"})
		)
	);
}
#[rstest]
#[case::wrong_run("remote.tool.completed",json!({"run_id":Uuid::nil(),"call":{"id":"call","name":"tool"}}),true)]
#[case::unsupported("arbitrary.event",json!({"run_id":Uuid::from_u128(1)}),true)]
#[case::long_phase("remote.run.recovered",json!({"run_id":Uuid::from_u128(1),"phase":"x".repeat(33),"cause":"expired worker lease"}),false)]
#[case::wrong_cause("remote.run.recovered",json!({"run_id":Uuid::from_u128(1),"phase":"resume","cause":"other"}),false)]
#[case::long_name("remote.tool.completed",json!({"run_id":Uuid::from_u128(1),"call":{"id":"call","name":"x".repeat(257)}}),false)]
#[case::long_id("remote.tool.completed",json!({"run_id":Uuid::from_u128(1),"call":{"id":"x".repeat(257),"name":"tool"}}),false)]
#[case::missing_phase("remote.run.recovered",json!({"run_id":Uuid::from_u128(1),"cause":"expired worker lease"}),false)]
fn unbound_or_malformed_audit_events_are_rejected(
	#[case] kind: &str,
	#[case] payload: Value,
	#[case] forbidden: bool,
) {
	let result = audit_event(&json!({"kind":kind,"data":payload}), Uuid::from_u128(1));
	if forbidden {
		assert!(matches!(result, Err(AuditError::Unbound)));
	} else {
		assert!(matches!(
			result,
			Err(AuditError::Invalid(Error::Invalid(_)))
		));
	}
}
#[rstest]
fn byte_limits_accept_the_original_inclusive_boundary() {
	let run = Uuid::from_u128(1);
	let input = json!({"kind":"remote.tool.completed","data":{"run_id":run,"call":{"id":"x".repeat(256),"name":"x".repeat(256)}}});
	assert!(audit_event(&input, run).is_ok());
	let input = json!({"kind":"remote.run.recovered","data":{"run_id":run,"phase":"x".repeat(32),"cause":"expired worker lease"}});
	assert!(audit_event(&input, run).is_ok());
}
