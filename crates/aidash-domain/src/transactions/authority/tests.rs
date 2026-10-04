use super::*;
use rstest::{fixture, rstest};

#[rstest]
#[case::valid("valid", true)]
#[case::caller("caller", false)]
#[case::targets("targets", false)]
#[case::empty("empty", false)]
#[case::recipients("recipients", false)]
#[case::node("node", false)]
#[case::digest("digest", false)]
fn preflight_requires_the_named_coordinator_bounded_frontier_and_local_recipient(
	#[case] change: &str,
	#[case] expected: bool,
) {
	let mut input = Preflight {
		id: Uuid::from_u128(1),
		coordinator: "aidash://home".into(),
		digest: "a".repeat(64),
		origin: Origin {
			credential_id: Uuid::nil(),
			tenant: "source".into(),
			subject: "origin".into(),
		},
		recipients: vec!["aidash://home".into()],
		targets: vec![],
	};
	let mut caller = "aidash://home";
	match change {
		"valid" => {}
		"caller" => caller = "aidash://other",
		"targets" => {
			input.targets = vec![
				Target {
					kind: "workspace".into(),
					id: Uuid::nil(),
					task_id: None
				};
				65
			]
		}
		"empty" => input.recipients.clear(),
		"recipients" => input.recipients = vec!["aidash://home".into(); 17],
		"node" => input.recipients = vec!["aidash://other".into()],
		"digest" => input.digest.pop().map(drop).unwrap(),
		_ => unreachable!(),
	}
	assert_eq!(input.permits(caller, "aidash://home"), expected);
}
#[fixture]
fn run() -> RunMetadata {
	serde_json::from_value(serde_json::json!({"id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),"home_node":"aidash://home","agent_id":"agent","agent_version":"1","phase":"READY","control":"ACTIVE","step":0,"revision":0,"observed_input_seq":0,"ledger_worker_ready":false,"error":null,"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z"})).unwrap()
}
#[rstest]
#[case::valid("valid", true)]
#[case::tenant("tenant", false)]
#[case::credential("credential", false)]
#[case::source_tenant("source_tenant", false)]
#[case::source_subject("source_subject", false)]
#[case::workspace("workspace", false)]
#[case::agent("agent", false)]
#[case::version("version", false)]
#[case::root("root", false)]
#[case::empty_chain("empty", false)]
fn source_admissions_require_the_current_credential_origin_and_assigned_agent(
	run: RunMetadata,
	#[case] change: &str,
	#[case] expected: bool,
) {
	let identity = ExecutionPrincipal {
		tenant: "executor".into(),
		subject: "root".into(),
		credential_id: Uuid::from_u128(4),
	};
	let origin = Origin {
		credential_id: Uuid::from_u128(5),
		tenant: "source".into(),
		subject: "origin".into(),
	};
	let mut admission = SourceAdmission {
		tenant: identity.tenant.clone(),
		credential_id: identity.credential_id,
		subject_chain: vec![identity.subject.clone(), "agent".into()],
		source_tenant: origin.tenant.clone(),
		source_subject: origin.subject.clone(),
		workspace_id: run.workspace_id,
		agent_id: run.agent_id.clone(),
		agent_version: run.agent_version.clone(),
	};
	match change {
		"valid" => {}
		"tenant" => admission.tenant = "other".into(),
		"credential" => admission.credential_id = Uuid::nil(),
		"source_tenant" => admission.source_tenant = "other".into(),
		"source_subject" => admission.source_subject = "other".into(),
		"workspace" => admission.workspace_id = Uuid::nil(),
		"agent" => admission.agent_id = "other".into(),
		"version" => admission.agent_version = "other".into(),
		"root" => admission.subject_chain[0] = "other".into(),
		"empty" => admission.subject_chain.clear(),
		_ => unreachable!(),
	}
	assert_eq!(admission.matches(&identity, &origin, &run), expected);
}
#[rstest]
#[case::empty(vec![], true)]
#[case::root(vec!["root".into()], true)]
#[case::same(vec!["root".into(),"agent".into()], true)]
#[case::different(vec!["root".into(),"other".into()], false)]
fn established_authority_chains_cannot_be_replaced(
	#[case] previous: Vec<String>,
	#[case] expected: bool,
) {
	assert_eq!(
		chain_compatible(&previous, &["root".into(), "agent".into()]),
		expected
	);
}
