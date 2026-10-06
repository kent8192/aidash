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

fn task() -> crate::Task {
	crate::Task {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
		title: "original title".into(),
		description: "original intent".into(),
		status: crate::TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "requester".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 7,
		created_at: chrono::TimeZone::timestamp_opt(&Utc, 1, 0).unwrap(),
	}
}
fn grant() -> Grant {
	Grant {
		id: Uuid::from_u128(3),
		task_id: task().id,
		task_revision: 7,
		workspace_id: task().workspace_id,
		node_id: "aidash://receiver".into(),
		tenant: "tenant".into(),
		credential_id: Uuid::from_u128(4),
		root_subject: "requester".into(),
		subject_chain: vec!["requester".into(), "executor".into()],
		inspection: json!({"inspection":"pinned"}),
		semantic: json!({"mode":"disabled"}),
		expires_at: chrono::TimeZone::timestamp_opt(&Utc, 3600, 0).unwrap(),
		revoked: false,
	}
}
fn bound() -> HomeBinding {
	HomeBinding {
		grant_id: grant().id,
		admission_id: Uuid::from_u128(5),
		task_id: task().id,
		task_revision: 8,
		initial_task: json!(task()),
	}
}
#[rstest]
#[case("task_id")]
#[case("task_revision")]
#[case("workspace_id")]
#[case("node_id")]
#[case("credential_id")]
#[case("tenant")]
#[case("root_subject")]
#[case("subject_chain")]
#[case("inspection")]
#[case("semantic")]
fn reused_grant_rejects_each_changed_authority_dimension(#[case] changed: &str) {
	let mut actual = grant();
	let task = task();
	let expected = grant();
	match changed {
		"task_id" => actual.task_id = Uuid::from_u128(99),
		"task_revision" => actual.task_revision += 1,
		"workspace_id" => actual.workspace_id = Uuid::from_u128(99),
		"node_id" => actual.node_id = "aidash://other".into(),
		"credential_id" => actual.credential_id = Uuid::from_u128(99),
		"tenant" => actual.tenant = "other".into(),
		"root_subject" => actual.root_subject = "other".into(),
		"subject_chain" => actual.subject_chain.reverse(),
		"inspection" => actual.inspection = json!({}),
		"semantic" => actual.semantic = json!({}),
		_ => panic!("unknown dimension"),
	}
	let input = super::super::PrepareInput {
		id: expected.id,
		node_id: expected.node_id.clone(),
		agent: EntityRef {
			id: "agent".into(),
			version: "1".into(),
		},
		ttl_seconds: 3600,
		semantic: Default::default(),
	};
	let identity = crate::identity::execution::ExecutionPrincipal {
		tenant: expected.tenant.clone(),
		subject: expected.root_subject.clone(),
		credential_id: expected.credential_id,
	};
	let authority = PreparationAuthority {
		task_id: task.id,
		task: &task,
		input: &input,
		identity: &identity,
		subjects: &expected.subject_chain,
		inspection: &expected.inspection,
		semantic: &expected.semantic,
	};
	assert!(expected.matches_authority(&authority));
	assert!(!actual.matches_authority(&authority));
}
#[rstest]
fn bound_command_retains_original_input_after_journal_revision_advances() {
	let grant = grant();
	let mut current = task();
	current.revision = 8;
	current.title = "advanced title".into();
	current.status = crate::TaskStatus::Running;
	let admitted = grant
		.admitted_task(&current, Some(&bound()))
		.unwrap()
		.unwrap();
	assert_eq!(admitted.title, "original title");
	assert_eq!(admitted.revision, 7);
	assert_eq!(admitted.status, crate::TaskStatus::Open);
}
#[rstest]
#[case("grant")]
#[case("task")]
#[case("original_task")]
#[case("workspace")]
#[case("original_revision")]
#[case("journal_revision")]
fn bound_description_rejects_replaced_identity_or_revision(#[case] changed: &str) {
	let mut binding = bound();
	let mut current = task();
	current.revision = 8;
	current.status = crate::TaskStatus::Running;
	match changed {
		"grant" => binding.grant_id = Uuid::from_u128(99),
		"task" => binding.task_id = Uuid::from_u128(99),
		"original_task" => binding.initial_task["id"] = json!(Uuid::from_u128(99)),
		"workspace" => binding.initial_task["workspace_id"] = json!(Uuid::from_u128(99)),
		"original_revision" => binding.initial_task["revision"] = json!(6),
		"journal_revision" => current.revision += 1,
		_ => panic!("unknown dimension"),
	}
	assert!(
		grant()
			.admitted_task(&current, Some(&binding))
			.unwrap()
			.is_none()
	);
}
#[rstest]
#[case(crate::TaskStatus::Claimed)]
#[case(crate::TaskStatus::Running)]
#[case(crate::TaskStatus::Completed)]
fn unbound_description_requires_an_open_task(#[case] status: crate::TaskStatus) {
	let mut current = task();
	current.status = status;
	assert!(grant().admitted_task(&current, None).unwrap().is_none());
}
#[rstest]
fn malformed_original_snapshot_preserves_decoding_failure() {
	let mut current = task();
	current.revision = 8;
	let mut binding = bound();
	binding.initial_task = json!({"bad":"snapshot"});
	assert!(grant().admitted_task(&current, Some(&binding)).is_err());
}
