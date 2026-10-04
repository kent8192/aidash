use super::*;
use aidash_domain::{
	Task,
	identity::execution::ExecutionPrincipal,
	policy::Resource,
	transactions::{Isolation, Participant, authority::SourceAdmission},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use uuid::Uuid;

#[fixture]
fn run() -> RunMetadata {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"task_id":Uuid::from_u128(2),"workspace_id":Uuid::from_u128(3),"home_node":"aidash://home","agent_id":"agent","agent_version":"1","phase":"READY","control":"ACTIVE","step":0,"revision":0,"observed_input_seq":0,"ledger_worker_ready":false,"error":null,"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z"})).unwrap()
}
#[fixture]
fn input() -> Preflight {
	Preflight {
		id: Uuid::from_u128(9),
		coordinator: "aidash://home".into(),
		digest: "manifest".into(),
		origin: Origin {
			credential_id: Uuid::from_u128(5),
			tenant: "source".into(),
			subject: "origin".into(),
		},
		recipients: vec!["aidash://home".into(), "aidash://peer".into()],
		targets: vec![
			Target {
				kind: "workspace".into(),
				id: Uuid::from_u128(3),
				task_id: None,
			},
			Target {
				kind: "task".into(),
				id: Uuid::from_u128(2),
				task_id: None,
			},
			Target {
				kind: "run".into(),
				id: Uuid::from_u128(1),
				task_id: Some(Uuid::from_u128(2)),
			},
		],
	}
}
struct Scope {
	run: RunMetadata,
	subjects: Vec<String>,
	calls: Vec<String>,
	remote: bool,
	missing: bool,
	mismatch: bool,
	failure: Option<String>,
}
#[fixture]
fn scope(run: RunMetadata) -> Scope {
	Scope {
		run,
		subjects: vec!["root".into()],
		calls: vec![],
		remote: false,
		missing: false,
		mismatch: false,
		failure: None,
	}
}
impl Scope {
	fn call(&mut self, name: String) -> Result<()> {
		self.calls.push(name.clone());
		if self.failure.as_deref() == Some(name.as_str()) {
			return Err(Error::External(format!("fault:{name}")));
		}
		Ok(())
	}
}
#[async_trait]
impl TransactionAuthorityScope for Scope {
	fn identity(&self) -> ExecutionPrincipal {
		ExecutionPrincipal {
			tenant: "executor".into(),
			subject: "root".into(),
			credential_id: Uuid::from_u128(4),
		}
	}
	fn source_node(&self) -> Option<&str> {
		self.remote.then_some("aidash://home")
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.subjects = subjects;
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		Resource {
			tenant: "executor".into(),
			kind: kind.into(),
			id: id.to_string(),
			attributes,
		}
	}
	fn qualified_resource(&self, kind: &str, node: &str, id: Uuid) -> Resource {
		Resource {
			tenant: "executor".into(),
			kind: kind.into(),
			id: format!("{node}:{id}"),
			attributes: json!({"node_id":node}),
		}
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		let name = if action == "transaction.disclose" {
			format!(
				"{action}:{}:{}",
				resource.kind,
				resource.attributes["recipient_node"].as_str().unwrap()
			)
		} else if let Some(node) = resource.attributes["node_id"].as_str() {
			format!("{action}:{}:{node}", resource.kind)
		} else {
			action.into()
		};
		self.call(name)
	}
	async fn inherit_task(&mut self, _: Uuid) -> Result<()> {
		self.call("inherit_task".into())?;
		self.subjects = vec!["root".into(), "agent".into()];
		Ok(())
	}
	async fn inherit_local_run(&mut self, _: &RunMetadata) -> Result<()> {
		self.call("inherit_local_run".into())?;
		self.subjects = vec!["root".into(), "agent".into()];
		Ok(())
	}
	async fn source_admission(
		&mut self,
		run: &RunMetadata,
		coordinator: &str,
	) -> Result<Option<SourceAdmission>> {
		assert_eq!(coordinator, "aidash://home");
		self.call("source_admission".into())?;
		if self.missing {
			return Ok(None);
		}
		Ok(Some(SourceAdmission {
			tenant: "executor".into(),
			credential_id: if self.mismatch {
				Uuid::nil()
			} else {
				Uuid::from_u128(4)
			},
			subject_chain: vec!["root".into(), "agent".into()],
			source_tenant: "source".into(),
			source_subject: "origin".into(),
			workspace_id: run.workspace_id,
			agent_id: run.agent_id.clone(),
			agent_version: run.agent_version.clone(),
		}))
	}
	async fn run(&mut self, _: Uuid) -> Result<RunMetadata> {
		self.call("run".into())?;
		Ok(self.run.clone())
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.call("workspace".into())?;
		Ok(self.resource("workspace", id, json!({})))
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		self.call("task".into())?;
		Ok(Task {
			id,
			workspace_id: self.run.workspace_id,
			title: String::new(),
			description: String::new(),
			status: aidash_domain::TaskStatus::Claimed,
			requirements: json!({}),
			owner: Some("agent".into()),
			created_by: "root".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 0,
			created_at: chrono::Utc::now(),
		})
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.call("task_resource".into())?;
		Ok(self.resource("task", task.id, json!({})))
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, owner: &str) -> Result<Resource> {
		assert_eq!(owner, "agent");
		self.call("artifact_resource".into())?;
		Ok(self.resource("artifact", task, json!({})))
	}
}
#[rstest]
#[tokio::test]
async fn all_origins_precede_mutation_checks_and_disclosures_keep_recipient_order(
	mut scope: Scope,
	input: Preflight,
) {
	checks(&mut scope, &input, "transaction.prepare")
		.await
		.unwrap();
	assert_eq!(
		scope.calls,
		vec![
			"transaction.prepare",
			"inherit_task",
			"run",
			"inherit_local_run",
			"transaction.prepare",
			"workspace",
			"workspace.read",
			"workspace.update",
			"transaction.disclose:workspace:aidash://home",
			"transaction.disclose:workspace:aidash://peer",
			"task",
			"task_resource",
			"task.complete",
			"artifact_resource",
			"artifact.create",
			"transaction.disclose:task:aidash://home",
			"transaction.disclose:task:aidash://peer",
			"run",
			"run.read",
			"run.finish",
			"transaction.disclose:run:aidash://home",
			"transaction.disclose:run:aidash://peer"
		]
	);
	assert_eq!(scope.subjects, vec!["root", "agent"]);
}
#[rstest]
#[tokio::test]
async fn read_checks_inherit_authority_without_mutation_or_disclosure_permissions(
	mut scope: Scope,
	input: Preflight,
) {
	checks(&mut scope, &input, "transaction.read")
		.await
		.unwrap();
	assert_eq!(
		scope.calls,
		vec![
			"transaction.read",
			"inherit_task",
			"run",
			"inherit_local_run",
			"transaction.read",
			"workspace",
			"workspace.read",
			"task",
			"task_resource",
			"run",
			"run.read"
		]
	);
}
#[rstest]
#[case::initial("transaction.prepare")]
#[case::origin("inherit_task")]
#[case::run("run")]
#[case::inherit("inherit_local_run")]
#[case::workspace("workspace.update")]
#[case::disclosure("transaction.disclose:workspace:aidash://peer")]
#[case::artifact("artifact.create")]
#[tokio::test]
async fn errors_stop_authority_checks_before_later_targets_and_preserve_error_identity(
	mut scope: Scope,
	input: Preflight,
	#[case] failure: &str,
) {
	scope.failure = Some(failure.into());
	let result = checks(&mut scope, &input, "transaction.prepare").await;
	assert!(matches!(result,Err(Error::External(message)) if message==format!("fault:{failure}")));
	assert_eq!(scope.calls.last().unwrap(), failure);
}
#[rstest]
#[tokio::test]
async fn a_run_cannot_inherit_authority_for_a_different_task(
	mut scope: Scope,
	mut input: Preflight,
) {
	input.targets = vec![input.targets[2].clone()];
	input.targets[0].task_id = Some(Uuid::nil());
	assert!(matches!(
		checks(&mut scope, &input, "transaction.prepare").await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["transaction.prepare", "run"]);
}
#[rstest]
#[case::present(false, false, true)]
#[case::missing(true, false, false)]
#[case::wrong_credential(false, true, false)]
#[tokio::test]
async fn home_source_preflight_requires_its_durable_admission(
	mut scope: Scope,
	input: Preflight,
	#[case] missing: bool,
	#[case] mismatch: bool,
	#[case] permitted: bool,
) {
	scope.remote = true;
	scope.missing = missing;
	scope.mismatch = mismatch;
	let run = scope.run.clone();
	let result = inherit_run(&mut scope, &run, &input).await;
	assert_eq!(result.is_ok(), permitted);
	if !permitted {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(scope.subjects, vec!["root"]);
	}
	assert_eq!(scope.calls, vec!["source_admission"]);
}
#[rstest]
#[case::local(false)]
#[case::remote(true)]
#[tokio::test]
async fn a_previous_delegation_chain_cannot_be_replaced_by_another_run(
	mut scope: Scope,
	input: Preflight,
	#[case] remote: bool,
) {
	scope.remote = remote;
	scope.subjects = vec!["root".into(), "other".into()];
	let run = scope.run.clone();
	assert!(matches!(
		inherit_run(&mut scope, &run, &input).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[tokio::test]
async fn unknown_targets_fail_before_resource_reads(mut scope: Scope, mut input: Preflight) {
	input.targets = vec![Target {
		kind: "registry".into(),
		id: Uuid::nil(),
		task_id: None,
	}];
	assert!(
		matches!(checks(&mut scope,&input,"transaction.prepare").await,Err(Error::Invalid(message)) if message=="unsupported transaction target")
	);
	assert_eq!(scope.calls, vec!["transaction.prepare"]);
}
#[rstest]
fn preflight_discloses_only_identifiers_but_digests_the_full_manifest(input: Preflight) {
	let manifest = Manifest {
		id: input.id,
		coordinator: input.coordinator.clone(),
		isolation: Isolation::Serializable,
		deadline: chrono::Utc::now(),
		participants: vec![Participant {
			node_id: input.coordinator.clone(),
			mutations: vec![
				Mutation::WorkspaceState {
					workspace_id: Uuid::from_u128(3),
					expected_revision: 8,
					state: json!({"private":"content"}),
				},
				Mutation::FinishRun {
					run_id: Uuid::from_u128(1),
					task_id: Uuid::from_u128(2),
					expected_revision: 9,
				},
			],
		}],
	};
	let preflight = request(&manifest, &input.origin, &manifest.coordinator).unwrap();
	assert_eq!(
		preflight.targets,
		vec![input.targets[0].clone(), input.targets[2].clone()]
	);
	assert_eq!(preflight.digest, manifest.digest().unwrap());
	assert_eq!(preflight.origin, input.origin);
	assert!(matches!(
		request(&manifest, &preflight.origin, "aidash://missing"),
		Err(Error::Forbidden)
	));
}

fn source_manifest(input: &Preflight) -> Manifest {
	Manifest {
		id: input.id,
		coordinator: input.coordinator.clone(),
		isolation: Isolation::Serializable,
		deadline: chrono::Utc::now(),
		participants: vec![
			Participant {
				node_id: input.coordinator.clone(),
				mutations: vec![Mutation::WorkspaceState {
					workspace_id: Uuid::from_u128(3),
					expected_revision: 0,
					state: json!({}),
				}],
			},
			Participant {
				node_id: "aidash://peer".into(),
				mutations: vec![Mutation::WorkspaceState {
					workspace_id: Uuid::from_u128(7),
					expected_revision: 0,
					state: json!({"private":"source"}),
				}],
			},
		],
	}
}
#[rstest]
#[case::read("transaction.read", false)]
#[case::submit("transaction.submit", true)]
#[tokio::test]
async fn remote_resources_require_source_permission_and_ordered_recipient_disclosure(
	mut scope: Scope,
	input: Preflight,
	#[case] action: &str,
	#[case] disclosures: bool,
) {
	let manifest = source_manifest(&input);
	source_checks(&mut scope, &manifest, &input.origin, action)
		.await
		.unwrap();
	let qualified = format!("{action}:workspace:aidash://peer");
	let start = scope
		.calls
		.iter()
		.position(|call| call == &qualified)
		.unwrap();
	let expected = if disclosures {
		vec![
			qualified,
			"transaction.disclose:workspace:aidash://home".into(),
			"transaction.disclose:workspace:aidash://peer".into(),
		]
	} else {
		vec![qualified]
	};
	assert_eq!(scope.calls[start..], expected);
}
#[rstest]
#[tokio::test]
async fn denied_source_permission_stops_remote_disclosure(mut scope: Scope, input: Preflight) {
	let manifest = source_manifest(&input);
	let failure = "transaction.submit:workspace:aidash://peer";
	scope.failure = Some(failure.into());
	assert!(
		matches!(source_checks(&mut scope,&manifest,&input.origin,"transaction.submit").await,Err(Error::External(message)) if message==format!("fault:{failure}"))
	);
	assert_eq!(scope.calls.last().unwrap(), failure);
}
