use super::*;
use aidash_domain::{
	identity::execution::{ExecutionPrincipal, TaskOrigin},
	policy::Resource,
	qualified_agent,
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::Mutex;

fn fixture() -> (RunMetadata, ExecutionGrant, ExecutionPrincipal) {
	let run = RunMetadata {
		id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		workspace_id: Uuid::from_u128(3),
		home_node: "aidash://node".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		phase: aidash_domain::RunPhase::Ready,
		control: aidash_domain::RunControl::Active,
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

struct Repository {
	current: Option<ExecutionGrant>,
	calls: Mutex<Vec<&'static str>>,
	fail: Option<&'static str>,
}
impl Repository {
	fn touch(&self, call: &'static str) -> Result<()> {
		self.calls.lock().unwrap().push(call);
		if self.fail == Some(call) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"authority adapter fault",
			))));
		}
		Ok(())
	}
}
#[async_trait]
impl ExecutionGrantRepository for Repository {
	fn node_id(&self) -> &str {
		"aidash://node"
	}
	async fn grant(&self, _: Uuid) -> Result<Option<ExecutionGrant>> {
		self.touch("grant")?;
		Ok(self.current.clone())
	}
	async fn require_legacy_remote_task(&self, _: &str, _: Uuid) -> Result<()> {
		self.touch("legacy_task")
	}
	async fn require_legacy_execution(&self, _: Uuid) -> Result<()> {
		self.touch("legacy_workspace")
	}
	async fn require_legacy_agent(&self, _: &str, _: &str) -> Result<()> {
		self.touch("legacy_agent")
	}
}
struct Scope {
	current: Option<ExecutionGrant>,
	identity: ExecutionPrincipal,
	subjects: Vec<String>,
	calls: Vec<&'static str>,
	selected: Option<(Uuid, Option<bool>)>,
	context: Value,
	local: Option<TaskOrigin>,
	remote: Option<TaskOrigin>,
	denied: bool,
	fail: Option<&'static str>,
}
impl Default for Scope {
	fn default() -> Self {
		let (_, grant, identity) = fixture();
		Self {
			current: Some(grant),
			identity,
			subjects: vec!["root".into()],
			calls: vec![],
			selected: None,
			context: json!({}),
			local: None,
			remote: None,
			denied: false,
			fail: None,
		}
	}
}
impl Scope {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call);
		if self.fail == Some(call) {
			return Err(Error::Port(Box::new(std::io::Error::other(
				"authority adapter fault",
			))));
		}
		Ok(())
	}
}
#[async_trait]
impl ExecutionGrantSession for Scope {
	fn identity(&self) -> ExecutionPrincipal {
		self.identity.clone()
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.calls.push("subjects");
		self.subjects = subjects;
	}
	fn select_worker(&mut self, run: Uuid, durable: Option<bool>) {
		self.calls.push("worker");
		self.selected = Some((run, durable));
	}
	async fn refresh(&mut self, _: Uuid) -> Result<()> {
		self.touch("refresh")
	}
	async fn required_grant(&mut self, _: Uuid) -> Result<ExecutionGrant> {
		self.touch("required_grant")?;
		self.current
			.clone()
			.ok_or_else(|| Error::Port(Box::new(std::io::Error::other("missing persisted grant"))))
	}
	async fn optional_grant(&mut self, _: Uuid) -> Result<Option<ExecutionGrant>> {
		self.touch("optional_grant")?;
		Ok(self.current.clone())
	}
	async fn local_origin(&mut self, _: Uuid) -> Result<Option<TaskOrigin>> {
		self.touch("local_origin")?;
		Ok(self.local.clone())
	}
	async fn remote_origin(&mut self, _: Uuid) -> Result<Option<TaskOrigin>> {
		self.touch("remote_origin")?;
		Ok(self.remote.clone())
	}
	async fn workspace(&mut self, _: Uuid) -> Result<Resource> {
		self.touch("workspace")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: fixture().0.workspace_id.to_string(),
			attributes: json!({"workspace":"context"}),
		})
	}
	fn set_context(&mut self, context: Value) {
		self.calls.push("context");
		self.context = context;
	}
	async fn require(&mut self, _: &Resource, action: &str) -> Result<()> {
		assert_eq!(action, "workspace.read");
		self.touch("require")?;
		if self.denied {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
}

#[rstest]
#[case::scoped(true)]
#[case::legacy(false)]
#[tokio::test]
async fn worker_admission_keeps_legacy_guards_in_the_original_order(#[case] scoped: bool) {
	let (run, binding, _) = fixture();
	let repository = Repository {
		current: scoped.then_some(binding),
		calls: Mutex::new(vec![]),
		fail: None,
	};
	assert_eq!(grant(&repository, &run).await.unwrap().is_some(), scoped);
	assert_eq!(
		*repository.calls.lock().unwrap(),
		if scoped {
			vec!["grant"]
		} else {
			vec!["grant", "legacy_task", "legacy_workspace", "legacy_agent"]
		}
	);
}

#[rstest]
#[tokio::test]
async fn invalid_scoped_binding_never_falls_back_to_legacy_access() {
	let (run, mut binding, _) = fixture();
	binding.workspace_id = Uuid::from_u128(9);
	let repository = Repository {
		current: Some(binding),
		calls: Mutex::new(vec![]),
		fail: None,
	};
	assert!(matches!(
		grant(&repository, &run).await,
		Err(Error::Forbidden)
	));
	assert_eq!(*repository.calls.lock().unwrap(), ["grant"]);
}

#[rstest]
#[case::grant("grant")]
#[case::legacy_task("legacy_task")]
#[case::legacy_workspace("legacy_workspace")]
#[case::legacy_agent("legacy_agent")]
#[tokio::test]
async fn legacy_admission_preserves_adapter_error_identity(#[case] fail: &'static str) {
	let repository = Repository {
		current: None,
		calls: Mutex::new(vec![]),
		fail: Some(fail),
	};
	let Err(Error::Port(error)) = grant(&repository, &fixture().0).await else {
		panic!("expected original port error")
	};
	assert!(error.downcast_ref::<std::io::Error>().is_some());
	assert_eq!(repository.calls.lock().unwrap().last(), Some(&fail));
}

#[rstest]
#[case::durable(true)]
#[case::transient(false)]
#[tokio::test]
async fn binding_retains_every_delegator_and_requires_the_workspace(#[case] durable: bool) {
	let (run, initial, _) = fixture();
	let mut scope = Scope::default();
	bind_worker(&mut scope, &run, &initial, durable)
		.await
		.unwrap();
	assert_eq!(scope.subjects, initial.subject_chain);
	assert_eq!(scope.selected, Some((run.id, Some(durable))));
	assert_eq!(scope.context, json!({"workspace":"context"}));
	assert_eq!(
		scope.calls,
		[
			"required_grant",
			"subjects",
			"worker",
			"workspace",
			"context",
			"require"
		]
	);
}

#[rstest]
#[case::credential(true)]
#[case::delegator(false)]
#[tokio::test]
async fn changed_authority_retries_before_workspace_or_worker_selection(#[case] credential: bool) {
	let (run, initial, _) = fixture();
	let mut scope = Scope::default();
	if credential {
		scope.current.as_mut().unwrap().credential_id = Uuid::from_u128(9);
	} else {
		scope.current.as_mut().unwrap().subject_chain[1] = "replacement".into();
	}
	assert!(
		matches!(bind_worker(&mut scope,&run,&initial,true).await,Err(Error::External(message)) if message=="execution authority changed; retry boundary")
	);
	assert_eq!(scope.calls, ["required_grant"]);
	assert!(scope.selected.is_none());
}

#[rstest]
#[tokio::test]
async fn missing_required_grants_preserve_storage_failure_identity() {
	let mut scope = Scope {
		current: None,
		..Default::default()
	};
	assert!(matches!(
		bind_worker(&mut scope, &fixture().0, &fixture().1, true).await,
		Err(Error::Port(_))
	));
	assert_eq!(scope.calls, ["required_grant"]);
}

#[rstest]
#[case::binding(false)]
#[case::refresh(true)]
#[tokio::test]
async fn authority_binding_cannot_bypass_a_workspace_denial(#[case] refreshed: bool) {
	let mut scope = Scope {
		denied: true,
		..Default::default()
	};
	let result = if refreshed {
		refresh_worker(&mut scope, "aidash://node", &fixture().0).await
	} else {
		bind_worker(&mut scope, &fixture().0, &fixture().1, true).await
	};
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls.last(), Some(&"require"));
}

#[rstest]
#[tokio::test]
async fn management_inheritance_preserves_the_full_grant_without_worker_side_effects() {
	let mut scope = Scope::default();
	scope.identity.credential_id = Uuid::from_u128(9);
	inherit_run(&mut scope, &fixture().0).await.unwrap();
	assert_eq!(scope.subjects, fixture().1.subject_chain);
	assert_eq!(scope.calls, ["optional_grant", "subjects"]);
	assert!(scope.selected.is_none());
}

#[rstest]
#[case::missing(false)]
#[case::mismatch(true)]
#[tokio::test]
async fn management_inheritance_retains_the_existing_visibility_errors(#[case] mismatch: bool) {
	let mut scope = Scope::default();
	if mismatch {
		scope.identity.subject = "other".into();
	} else {
		scope.current = None;
	}
	let result = inherit_run(&mut scope, &fixture().0).await;
	if mismatch {
		assert!(matches!(result,Err(Error::NotFound(message)) if message=="run unavailable"));
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(scope.calls, ["optional_grant"]);
}

#[rstest]
#[tokio::test]
async fn refresh_locks_policy_and_credential_before_the_current_grant() {
	let mut scope = Scope::default();
	refresh_worker(&mut scope, "aidash://node", &fixture().0)
		.await
		.unwrap();
	assert_eq!(
		scope.calls,
		[
			"refresh",
			"optional_grant",
			"subjects",
			"worker",
			"workspace",
			"context",
			"require"
		]
	);
	assert_eq!(scope.subjects, fixture().1.subject_chain);
	assert_eq!(scope.selected, Some((fixture().0.id, None)));
}

#[rstest]
#[tokio::test]
async fn refreshed_credential_mismatch_never_selects_a_worker() {
	let mut scope = Scope::default();
	scope.identity.credential_id = Uuid::from_u128(9);
	assert!(
		matches!(refresh_worker(&mut scope,"aidash://node",&fixture().0).await,Err(Error::External(message)) if message=="execution authority changed; retry boundary")
	);
	assert_eq!(scope.calls, ["refresh", "optional_grant"]);
	assert!(scope.selected.is_none());
}

#[rstest]
#[case::local(true, false)]
#[case::remote(false, true)]
#[case::missing(false, false)]
#[tokio::test]
async fn task_origin_prefers_local_provenance_before_remote_fallback(
	#[case] local: bool,
	#[case] remote: bool,
) {
	let (_, binding, _) = fixture();
	let origin = TaskOrigin {
		tenant: binding.tenant,
		root_subject: binding.root_subject,
		subject_chain: binding.subject_chain.clone(),
	};
	let mut scope = Scope {
		local: local.then_some(origin.clone()),
		remote: remote.then_some(origin),
		..Default::default()
	};
	assert_eq!(
		inherit_task(&mut scope, fixture().0.task_id).await.unwrap(),
		local || remote
	);
	assert_eq!(
		scope.calls,
		if local {
			vec!["local_origin", "subjects"]
		} else if remote {
			vec!["local_origin", "remote_origin", "subjects"]
		} else {
			vec!["local_origin", "remote_origin"]
		}
	);
	assert_eq!(
		scope.subjects,
		if local || remote {
			binding.subject_chain
		} else {
			vec!["root".into()]
		}
	);
}

#[rstest]
#[tokio::test]
async fn task_origin_cannot_replace_an_already_delegated_authority() {
	let (_, binding, _) = fixture();
	let origin = TaskOrigin {
		tenant: binding.tenant,
		root_subject: binding.root_subject,
		subject_chain: binding.subject_chain,
	};
	let mut scope = Scope {
		subjects: vec!["root".into(), "different-delegator".into()],
		local: Some(origin),
		..Default::default()
	};
	assert!(matches!(
		inherit_task(&mut scope, fixture().0.task_id).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, ["local_origin"]);
	assert_eq!(scope.subjects, ["root", "different-delegator"]);
}

#[rstest]
#[case::refresh("refresh")]
#[case::grant("optional_grant")]
#[case::workspace("workspace")]
#[case::decision("require")]
#[tokio::test]
async fn refresh_preserves_scoped_adapter_errors(#[case] fail: &'static str) {
	let mut scope = Scope {
		fail: Some(fail),
		..Default::default()
	};
	let Err(Error::Port(error)) = refresh_worker(&mut scope, "aidash://node", &fixture().0).await
	else {
		panic!("expected original port error")
	};
	assert!(error.downcast_ref::<std::io::Error>().is_some());
	assert_eq!(scope.calls.last(), Some(&fail));
}

#[rstest]
#[case::agent("agent", true)]
#[case::user("user", false)]
#[case::missing("missing", false)]
fn execution_admission_requires_an_agent_subject(#[case] subject: &str, #[case] permitted: bool) {
	let bundle:PolicyBundle=serde_json::from_value(json!({"tenant":"tenant","subjects":{"agent":{"kind":"agent","delegated_by":null},"user":{"kind":"user","delegated_by":null}}})).unwrap();
	assert_eq!(require_agent(&bundle, subject).is_ok(), permitted);
}
