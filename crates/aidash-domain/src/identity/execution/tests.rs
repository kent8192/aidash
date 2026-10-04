use super::*;
use rstest::rstest;

fn fixture() -> (RunMetadata, ExecutionGrant, ExecutionPrincipal) {
	let run = RunMetadata {
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		home_node: "aidash://node".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		phase: crate::RunPhase::Ready,
		control: crate::RunControl::Active,
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	};
	let grant = ExecutionGrant {
		run_id: run.id,
		task_id: run.task_id,
		workspace_id: run.workspace_id,
		tenant: "tenant".into(),
		credential_id: Uuid::from_u128(4),
		root_subject: "root".into(),
		subject_chain: vec![
			"root".into(),
			"delegator".into(),
			qualified_agent(&run.home_node, &run.agent_id, &run.agent_version),
		],
	};
	let identity = ExecutionPrincipal {
		tenant: "tenant".into(),
		subject: "root".into(),
		credential_id: grant.credential_id,
	};
	(run, grant, identity)
}

#[rstest]
#[case::valid("valid", true)]
#[case::home("home", false)]
#[case::run("run", false)]
#[case::task("task", false)]
#[case::workspace("workspace", false)]
#[case::root("root", false)]
#[case::agent("agent", false)]
#[case::missing_chain("missing", false)]
fn worker_grants_require_the_admitted_node_and_complete_chain(
	#[case] change: &str,
	#[case] expected: bool,
) {
	let (mut run, mut grant, _) = fixture();
	match change {
		"valid" => {}
		"home" => run.home_node = "aidash://foreign".into(),
		"run" => grant.run_id = Uuid::from_u128(9),
		"task" => grant.task_id = Uuid::from_u128(9),
		"workspace" => grant.workspace_id = Uuid::from_u128(9),
		"root" => grant.subject_chain[0] = "replacement".into(),
		"agent" => grant.subject_chain.pop().map(drop).unwrap(),
		"missing" => grant.subject_chain.clear(),
		_ => unreachable!(),
	}
	assert_eq!(grant.matches_worker("aidash://node", &run), expected);
}

#[rstest]
fn credential_rotation_is_allowed_for_management_inheritance_and_fenced_for_workers() {
	let (run, grant, mut identity) = fixture();
	identity.credential_id = Uuid::from_u128(9);
	assert!(grant.matches_inherited(&identity, &run));
	assert!(!grant.matches_refreshed("aidash://node", &identity, &run));
}

#[rstest]
#[case::valid("valid", true)]
#[case::tenant("tenant", false)]
#[case::root("root", false)]
#[case::credential("credential", false)]
#[case::task("task", false)]
#[case::workspace("workspace", false)]
#[case::agent("agent", false)]
#[case::chain("chain", false)]
fn refresh_checks_the_current_identity_and_qualified_agent(
	#[case] change: &str,
	#[case] expected: bool,
) {
	let (run, mut grant, _) = fixture();
	let identity = fixture().2;
	match change {
		"valid" => {}
		"tenant" => grant.tenant = "other".into(),
		"root" => grant.root_subject = "other".into(),
		"credential" => grant.credential_id = Uuid::from_u128(9),
		"task" => grant.task_id = Uuid::from_u128(9),
		"workspace" => grant.workspace_id = Uuid::from_u128(9),
		"agent" => *grant.subject_chain.last_mut().unwrap() = "old-agent".into(),
		"chain" => grant.subject_chain.clear(),
		_ => unreachable!(),
	}
	assert_eq!(
		grant.matches_refreshed("aidash://node", &identity, &run),
		expected
	);
}

#[rstest]
#[case::same(false, false, true)]
#[case::credential(true, false, false)]
#[case::delegator(false, true, false)]
fn source_rotation_retries_even_when_the_final_agent_is_unchanged(
	#[case] credential: bool,
	#[case] delegator: bool,
	#[case] expected: bool,
) {
	let (_, initial, _) = fixture();
	let mut current = initial.clone();
	if credential {
		current.credential_id = Uuid::from_u128(9);
	}
	if delegator {
		current.subject_chain[1] = "replacement-delegator".into();
	}
	assert_eq!(current.same_source(&initial), expected);
}

#[rstest]
#[case::root_only(1, false, true)]
#[case::exact(3, false, true)]
#[case::different_delegation(3, true, false)]
fn task_origins_preserve_an_existing_delegated_chain(
	#[case] length: usize,
	#[case] changed: bool,
	#[case] expected: bool,
) {
	let (_, grant, identity) = fixture();
	let origin = TaskOrigin {
		tenant: grant.tenant,
		root_subject: grant.root_subject,
		subject_chain: grant.subject_chain,
	};
	let mut current = origin.subject_chain[..length].to_vec();
	if changed {
		current[1] = "other".into();
	}
	assert_eq!(origin.compatible(&identity, &current), expected);
}
