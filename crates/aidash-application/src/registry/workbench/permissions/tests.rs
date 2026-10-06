use super::*;
use crate::ports::registry::workbench::permissions::PermissionScope;
use aidash_domain::{policy::Decision, registry::Entry};
use async_trait::async_trait;
use rstest::rstest;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
struct State {
	active: bool,
	committed: bool,
	calls: Vec<String>,
	evaluations: Vec<Evaluation>,
}

struct Repository {
	principal: Principal,
	state: Mutex<State>,
	inspection_allowed: bool,
	agent_kind: &'static str,
	catalog: Option<bool>,
	action_allowed: bool,
	registry_allowed: bool,
	owner: Option<String>,
	pause: bool,
}

impl Repository {
	fn new() -> Self {
		Self {
			principal: Principal::Subject {
				tenant: "tenant".into(),
				subject: "reader".into(),
			},
			state: Mutex::new(State::default()),
			inspection_allowed: true,
			agent_kind: "agent",
			catalog: Some(true),
			action_allowed: true,
			registry_allowed: true,
			owner: Some("owner".into()),
			pause: false,
		}
	}
}

struct Scope<'a>(&'a Repository);
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.0.state.lock().unwrap().active = false;
	}
}

fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1".into(),
	}
}
fn input() -> PermissionInput {
	PermissionInput {
		tenant: "tenant".into(),
		subject: "reader".into(),
		workspace_id: None,
	}
}

#[async_trait]
impl PermissionRepository for Repository {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	fn node_id(&self) -> &str {
		"node"
	}
	async fn begin(&self) -> Result<Box<dyn PermissionScope + '_>> {
		let mut s = self.state.lock().unwrap();
		s.active = true;
		s.calls.push("begin".into());
		Ok(Box::new(Scope(self)))
	}
}

#[async_trait]
impl PermissionScope for Scope<'_> {
	async fn require_inspection(&mut self, entry: &EntityRef) -> Result<()> {
		assert_eq!(*entry, reference("agent"));
		self.0.state.lock().unwrap().calls.push("inspection".into());
		if self.0.inspection_allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn effective(&mut self, entry: &EntityRef) -> Result<Entry> {
		self.0
			.state
			.lock()
			.unwrap()
			.calls
			.push(format!("effective:{}", entry.id));
		let kind = if entry.id == "agent" {
			self.0.agent_kind
		} else {
			entry.id.as_str()
		};
		let config = if entry.id == "agent" {
			json!({"model":reference("model"),"skills":[reference("skill")],"tools":[reference("tool")],"cluster":reference("cluster")})
		} else {
			json!({"fixture": entry.id})
		};
		Ok(serde_json::from_value(json!({
			"id":entry.id,"version":entry.version,"kind":kind,"name":{},"description":{},
			"capabilities":["capability"],"tags":["tag"],"languages":["en"],"config":config
		}))?)
	}

	async fn catalog_enabled(&mut self, tenant: &str, entry: &EntityRef) -> Result<Option<bool>> {
		assert_eq!(tenant, "tenant");
		assert_eq!(entry.version, "1");
		self.0
			.state
			.lock()
			.unwrap()
			.calls
			.push(format!("catalog:{}", entry.id));
		Ok(self.0.catalog)
	}

	async fn evaluate(&mut self, tenant: &str, evaluation: &Evaluation) -> Result<Decision> {
		assert_eq!(tenant, "tenant");
		{
			let mut s = self.0.state.lock().unwrap();
			assert!(s.active);
			s.calls.push(evaluation.action.clone());
			s.evaluations.push(evaluation.clone());
		}
		if self.0.pause {
			std::future::pending::<()>().await;
		}
		let registry = evaluation.action == "registry.read";
		Ok(Decision {
			allowed: if registry {
				self.0.registry_allowed
			} else {
				self.0.action_allowed
			},
			revision: if registry {
				999
			} else if evaluation.action == "workspace.read" {
				7
			} else {
				3
			},
			reason: "fixture".into(),
			matched_policies: vec![],
			effective_roles: Default::default(),
		})
	}

	async fn workspace_owner(&mut self, workspace: Uuid, tenant: &str) -> Result<Option<String>> {
		assert_eq!(tenant, "tenant");
		assert_eq!(workspace, Uuid::from_u128(1));
		self.0
			.state
			.lock()
			.unwrap()
			.calls
			.push("workspace_owner".into());
		Ok(self.0.owner.clone())
	}

	async fn commit(self: Box<Self>) -> Result<()> {
		let mut s = self.0.state.lock().unwrap();
		s.committed = true;
		s.calls.push("commit".into());
		Ok(())
	}
}

#[rstest]
#[case("other", "reader")]
#[case("tenant", "other")]
#[tokio::test]
async fn subject_cannot_inspect_another_execution_identity_before_begin(
	#[case] tenant: &str,
	#[case] subject: &str,
) {
	let repository = Repository::new();
	let mut selected = input();
	selected.tenant = tenant.into();
	selected.subject = subject.into();
	assert!(matches!(
		inspect(&repository, reference("agent"), selected).await,
		Err(Error::Forbidden)
	));
	assert!(repository.state.lock().unwrap().calls.is_empty());
}

#[rstest]
#[case(false, "agent", "inspection")]
#[case(true, "tool", "kind")]
#[tokio::test]
async fn current_inspection_authority_and_agent_kind_gate_component_reads(
	#[case] allowed: bool,
	#[case] kind: &'static str,
	#[case] denial: &str,
) {
	let mut repository = Repository::new();
	repository.inspection_allowed = allowed;
	repository.agent_kind = kind;
	let error = inspect(&repository, reference("agent"), input())
		.await
		.unwrap_err();
	if denial == "inspection" {
		assert!(matches!(error, Error::Forbidden));
	} else {
		assert!(matches!(error, Error::NotFound(ref value) if value == "agent version"));
	}
	let s = repository.state.lock().unwrap();
	assert!(!s.active && !s.committed);
	assert!(s.evaluations.is_empty());
	assert!(!s.calls.iter().any(|v| v.starts_with("catalog:")));
}

#[rstest]
#[tokio::test]
async fn component_actions_use_exact_pinned_metadata_and_worker_context() {
	let repository = Repository::new();
	let result = inspect(&repository, reference("agent"), input())
		.await
		.unwrap();
	assert_eq!(
		result
			.rows
			.iter()
			.map(|r| r.action.as_str())
			.collect::<Vec<_>>(),
		vec![
			"agent.execute",
			"model.infer",
			"skill.use",
			"tool.invoke",
			"cluster.execute"
		]
	);
	assert_eq!(result.rows[0].registry_read_allowed, None);
	assert!(
		result.rows[1..]
			.iter()
			.all(|r| r.registry_read_allowed == Some(true))
	);
	assert!(result.rows.iter().all(|r| r.effective_for_component));
	assert_eq!(result.requested_capabilities, vec!["capability"]);
	assert_eq!(result.policy_revision, 3);
	assert_eq!(result.workspace_read, None);
	let s = repository.state.lock().unwrap();
	assert!(!s.active && s.committed);
	assert_eq!(s.calls.last().map(String::as_str), Some("commit"));
	assert_eq!(s.evaluations.len(), 9);
	for e in &s.evaluations {
		assert_eq!(e.subject, "reader");
		assert_eq!(e.resource.tenant, "tenant");
		assert_eq!(e.resource.attributes["version"], "1");
		assert_eq!(e.resource.attributes["capabilities"], json!(["capability"]));
		assert_eq!(e.resource.attributes["tags"], json!(["tag"]));
		assert_eq!(e.resource.attributes["languages"], json!(["en"]));
		assert_eq!(
			e.environment,
			json!({"workspace_id":null,"node_id":"node","transport":"worker"})
		);
		if e.resource.id != "agent" {
			assert_eq!(
				e.resource.attributes["config"],
				json!({"fixture":e.resource.id})
			);
		}
	}
}

#[rstest]
#[case(None, true, true)]
#[case(Some(false), true, true)]
#[case(Some(true), false, true)]
#[case(Some(true), true, false)]
#[tokio::test]
async fn catalog_action_and_required_registry_read_each_gate_effective_permission(
	#[case] catalog: Option<bool>,
	#[case] action: bool,
	#[case] registry: bool,
) {
	let mut repository = Repository::new();
	repository.catalog = catalog;
	repository.action_allowed = action;
	repository.registry_allowed = registry;
	let result = inspect(&repository, reference("agent"), input())
		.await
		.unwrap();
	assert_eq!(
		result.rows[0].effective_for_component,
		catalog == Some(true) && action
	);
	assert!(result.rows[1..].iter().all(|r| !r.effective_for_component));
	assert_eq!(result.policy_revision, 3);
}

#[rstest]
#[case(Some("owner"), true, Some(true), 7)]
#[case(Some("owner"), false, Some(false), 7)]
#[case(None, true, Some(false), 3)]
#[tokio::test]
async fn workspace_permission_uses_current_owner_and_the_final_action_revision(
	#[case] owner: Option<&str>,
	#[case] allowed: bool,
	#[case] expected: Option<bool>,
	#[case] revision: i64,
) {
	let mut repository = Repository::new();
	repository.owner = owner.map(str::to_owned);
	repository.action_allowed = allowed;
	let mut selected = input();
	selected.workspace_id = Some(Uuid::from_u128(1));
	let result = inspect(&repository, reference("agent"), selected)
		.await
		.unwrap();
	assert_eq!(result.workspace_read, expected);
	assert_eq!(result.policy_revision, revision);
	let s = repository.state.lock().unwrap();
	let workspace = s.evaluations.iter().find(|v| v.action == "workspace.read");
	assert_eq!(workspace.is_some(), owner.is_some());
	if let Some(e) = workspace {
		assert_eq!(
			e.resource.attributes,
			json!({"owner":"owner","workspace_id":Uuid::from_u128(1)})
		);
		assert_eq!(
			e.environment,
			json!({"workspace_id":Uuid::from_u128(1),"node_id":"node","transport":"worker"})
		);
	}
}

#[rstest]
#[tokio::test]
async fn cancelling_policy_evaluation_releases_the_uncommitted_scope() {
	let mut repository = Repository::new();
	repository.pause = true;
	let mut future = Box::pin(inspect(&repository, reference("agent"), input()));
	assert!(futures_util::poll!(&mut future).is_pending());
	assert!(repository.state.lock().unwrap().active);
	drop(future);
	let s = repository.state.lock().unwrap();
	assert!(!s.active && !s.committed);
}
