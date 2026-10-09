use super::*;
use crate::ports::authorization::tools::{AgentToolRepository, AgentToolScope};
use aidash_domain::{
	HumanRequest, RunControl, RunPhase, Task, TaskStatus,
	policy::Resource,
	registry::{
		Entry,
		bindings::{
			BindingOrigin, BindingSnapshot, Narrowing, ResolvedBinding, ResolvedDefinition,
		},
	},
};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[rstest]
#[tokio::test]
async fn renamed_builtin_keeps_its_grant_resource_and_agent_flag(
	repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	let contract = aidash_domain::tool::builtin_contract("task_create").unwrap();
	let input = call("make_work", json!({}));
	super::authorize(&repository, &run, &agent, &input, &contract)
		.await
		.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease".to_owned(),
			"require:builtin:task_create:tool.invoke".to_owned(),
			format!("require:{}:task.create", run.workspace_id),
			"release".to_owned()
		]
	);
	agent.allow_task_creation = Some(false);
	let before = repository.calls();
	assert!(matches!(
		super::authorize(&repository, &run, &agent, &input, &contract).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls(), before);
}

#[rstest]
#[tokio::test]
async fn renamed_registry_alias_uses_exact_reference(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	let contract = ToolContract::registry(
		reference(),
		&ToolConfig::Native {
			operation: "echo".into(),
			allowed_hosts: vec![],
		},
	);
	super::authorize(
		&repository,
		&run,
		&agent,
		&call("no_index_here", json!({})),
		&contract,
	)
	.await
	.unwrap();
	assert_eq!(
		repository.calls(),
		["lease", "catalog:configured:tool.invoke", "release"]
	);
}

#[rstest]
#[tokio::test]
async fn remote_visibility_uses_contracts_after_aliases_change(
	mut repository: Repository,
	agent: AgentConfig,
) {
	repository.remote = true;
	let mut tools = BTreeMap::from([
		(
			"create_alias".into(),
			aidash_domain::tool::builtin_contract("task_create").unwrap(),
		),
		(
			"external_alias".into(),
			aidash_domain::tool::builtin_contract("outbound_get").unwrap(),
		),
		(
			"registered_alias".into(),
			ToolContract::registry(
				reference(),
				&ToolConfig::Native {
					operation: "echo".into(),
					allowed_hosts: vec![],
				},
			),
		),
	]);
	super::filter(&repository, &agent, &mut tools, Clone::clone)
		.await
		.unwrap();
	assert_eq!(
		tools.keys().map(String::as_str).collect::<Vec<_>>(),
		["create_alias", "registered_alias"]
	);
}

// Fixtures assemble contracts just as the native tool composition does.
async fn authorize(
	repository: &Repository,
	run: &RunMetadata,
	config: &AgentConfig,
	call: &ToolCall,
) -> Result<()> {
	let contract = aidash_domain::tool::builtin_contract(&call.name).or_else(|| {
		config
			.tools
			.iter()
			.enumerate()
			.find(|(index, _)| call.name == format!("plugin_{index}"))
			.map(|(_, reference)| {
				ToolContract::registry(
					reference.clone(),
					&ToolConfig::Native {
						operation: "echo".into(),
						allowed_hosts: vec![],
					},
				)
			})
	});
	let Some(contract) = contract else {
		refresh(repository).await?;
		return Err(Error::Forbidden);
	};
	super::authorize(repository, run, config, call, &contract).await
}
async fn filter<T: Send + Clone>(
	repository: &Repository,
	config: &AgentConfig,
	tools: &mut BTreeMap<String, T>,
) -> Result<()> {
	let mut assembled = tools
		.iter()
		.map(|(name, value)| {
			let contract = aidash_domain::tool::builtin_contract(name).unwrap_or_else(|| {
				ToolContract::registry(
					reference(),
					&serde_json::from_value(repository.catalog.config.clone()).unwrap(),
				)
			});
			(name.clone(), (value.clone(), contract))
		})
		.collect();
	let result = super::filter(repository, config, &mut assembled, |(_, contract)| {
		contract.clone()
	})
	.await;
	*tools = assembled
		.into_iter()
		.map(|(name, (value, _))| (name, value))
		.collect();
	result
}
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
fn reference() -> EntityRef {
	EntityRef {
		id: "configured".into(),
		version: "1".into(),
	}
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: id(1),
		task_id: id(2),
		workspace_id: id(3),
		home_node: "aidash://home".into(),
		agent_id: "producer".into(),
		agent_version: "1".into(),
		phase: RunPhase::Ready,
		control: RunControl::Active,
		step: 0,
		revision: 5,
		observed_input_seq: 0,
		ledger_worker_ready: false,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: Utc::now(),
	}
}
#[fixture]
fn agent() -> AgentConfig {
	let mut config: AgentConfig =
		serde_json::from_value(crate::test_support::agent("producer").config).unwrap();
	// Resource adapter flags are an internal view derived by native admission.
	config.memory = Some(reference());
	config.allow_memory_write = Some(true);
	config.tools = vec![reference()];
	config.skills = vec![reference()];
	config
}
fn call(name: &str, arguments: Value) -> ToolCall {
	ToolCall {
		id: "call".into(),
		name: name.into(),
		arguments,
	}
}
fn entry() -> Entry {
	Entry {
		binding_normalization: None,
		installation: None,
		id: "configured".into(),
		version: "1".into(),
		kind: "tool".into(),
		name: Default::default(),
		description: Default::default(),
		capabilities: vec![],
		tags: vec![],
		languages: vec![],
		skills: vec![],
		schema: json!({}),
		config: json!({"transport":"native","operation":"fixture"}),
	}
}
fn resource(kind: &str, key: &str, attributes: Value) -> Resource {
	Resource {
		tenant: "tenant".into(),
		kind: kind.into(),
		id: key.into(),
		attributes,
	}
}
#[derive(Default)]
struct Journal {
	calls: Vec<String>,
	decisions: Vec<(Resource, String)>,
}
#[derive(Clone, Copy)]
enum Outcome {
	Forbidden,
	Missing,
	Fault,
}
struct Repository {
	missing_request: bool,
	remote: bool,
	active: bool,
	replace: bool,
	fault: Option<&'static str>,
	stall: Option<String>,
	subjects: Vec<String>,
	context: Value,
	catalog: Entry,
	outcomes: BTreeMap<(String, String), Outcome>,
	journal: Arc<Mutex<Journal>>,
	snapshot: Option<BindingSnapshot>,
}
#[fixture]
fn repository() -> Repository {
	Repository {
		missing_request: false,
		remote: false,
		active: true,
		replace: true,
		fault: None,
		stall: None,
		subjects: vec!["root".into(), "delegated".into()],
		context: json!({"current":"authority"}),
		catalog: entry(),
		outcomes: BTreeMap::new(),
		journal: Arc::new(Mutex::new(Journal::default())),
		snapshot: None,
	}
}
impl Repository {
	fn record(&self, name: impl Into<String>) {
		self.journal.lock().unwrap().calls.push(name.into());
	}
	fn calls(&self) -> Vec<String> {
		self.journal.lock().unwrap().calls.clone()
	}
	fn decisions(&self) -> Vec<(Resource, String)> {
		self.journal.lock().unwrap().decisions.clone()
	}
	fn check(&self, stage: &'static str) -> Result<()> {
		if self.fault == Some(stage) {
			Err(Error::Port(Box::new(std::io::Error::other(format!(
				"{stage} fault"
			)))))
		} else {
			Ok(())
		}
	}
}
struct Scope<'a>(&'a Repository);
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.0.record("release");
	}
}
#[async_trait]
impl AgentToolRepository for Repository {
	fn is_remote(&self) -> bool {
		self.remote
	}
	fn binding_snapshot(&self) -> Option<&BindingSnapshot> {
		self.snapshot.as_ref()
	}
	async fn lease(&self) -> Result<Box<dyn AgentToolScope + '_>> {
		self.record("lease");
		self.check("lease")?;
		Ok(Box::new(Scope(self)))
	}
}
#[async_trait]
impl AgentToolScope for Scope<'_> {
	fn transaction_active(&self) -> bool {
		self.0.record("active");
		self.0.active
	}
	async fn suspend(&mut self) -> Result<()> {
		self.0.record("suspend");
		self.0.check("suspend")
	}
	async fn replace_remote_authority(&mut self) -> Result<bool> {
		self.0.record("replace");
		self.0.check("replace")?;
		Ok(self.0.replace)
	}
	fn context(&self) -> &Value {
		&self.0.context
	}
	fn subjects(&self) -> &[String] {
		&self.0.subjects
	}
	fn resource(&self, kind: &str, key: &str, attributes: Value) -> Resource {
		resource(kind, key, attributes)
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		self.0.record(format!("catalog:{}:{action}", reference.id));
		self.0.check("catalog")?;
		Ok(self.0.catalog.clone())
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.0.record(format!("require:{}:{action}", resource.id));
		self.0
			.journal
			.lock()
			.unwrap()
			.decisions
			.push((resource.clone(), action.into()));
		if self.0.stall.as_deref() == Some(action) {
			std::future::pending::<()>().await;
		}
		match self.0.outcomes.get(&(resource.id.clone(), action.into())) {
			Some(Outcome::Forbidden) => Err(Error::Forbidden),
			Some(Outcome::Missing) => Err(Error::NotFound("saved tool".into())),
			Some(Outcome::Fault) => Err(Error::Port(Box::new(std::io::Error::other(
				"require fault",
			)))),
			None => self.0.check("require"),
		}
	}
	async fn task_read(&mut self, key: Uuid) -> Result<Task> {
		self.0.record(format!("task_read:{key}"));
		self.0.check("task_read")?;
		Ok(Task {
			id: key,
			workspace_id: id(3),
			title: "saved".into(),
			description: String::new(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "saved creator".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 7,
			created_at: Utc::now(),
		})
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.0.record("task_resource");
		self.0.check("task_resource")?;
		Ok(resource(
			"task",
			&task.id.to_string(),
			json!({"revision":task.revision,"creator":task.created_by}),
		))
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, creator: &str) -> Result<Resource> {
		self.0.record(format!("artifact:{creator}"));
		self.0.check("artifact")?;
		Ok(resource(
			"artifact",
			&task.to_string(),
			json!({"created_by":creator}),
		))
	}
	async fn memory_resource(&mut self, run: &RunMetadata) -> Result<Resource> {
		self.0.record("memory");
		self.0.check("memory")?;
		Ok(resource(
			"memory",
			&run.agent_id,
			json!({"workspace":run.workspace_id,"run":run.id}),
		))
	}

	async fn human_request(&mut self, run: &RunMetadata, id: Uuid) -> Result<Option<HumanRequest>> {
		self.0
			.record(format!("human:{}:{}:{id}", run.id, run.workspace_id));
		self.0.check("human")?;
		Ok((!self.0.missing_request).then(|| HumanRequest {
			id,
			run_id: run.id,
			workspace_id: run.workspace_id,
			kind: "approval".into(),
			prompt: "saved question".into(),
			response: None,
			answered_by: None,
			created_at: Utc::now(),
		}))
	}
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
		self.0.record("human_resource");
		self.0.check("human_resource")?;
		Ok(resource(
			"human_request",
			&request.id.to_string(),
			json!({"run":request.run_id,"workspace":request.workspace_id,"prompt":request.prompt}),
		))
	}
}
fn assert_fault(error: Error, message: &str) {
	let Error::Port(error) = error else {
		panic!("expected adapter fault")
	};
	assert_eq!(
		error.downcast_ref::<std::io::Error>().unwrap().to_string(),
		message
	);
}
#[rstest]
#[case::create("task_create")]
#[case::delegate("task_delegate")]
#[case::memory("memory_mutate")]
#[case::retrieval("workspace_read")]
#[tokio::test]
async fn disabled_local_tool_flags_precede_acquiring_authority(
	repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] name: &str,
) {
	agent.allow_task_creation = Some(false);
	agent.allow_task_delegation = Some(false);
	agent.allow_memory_write = Some(false);
	agent.allow_workspace_retrieval = Some(false);
	assert!(matches!(
		authorize(&repository, &run, &agent, &call(name, json!({}))).await,
		Err(Error::Forbidden)
	));
	assert!(repository.calls().is_empty());
}
#[rstest]
#[case::active(true)]
#[case::suspended(false)]
#[tokio::test]
async fn remote_refresh_precedes_even_a_disabled_tool_flag(
	mut repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] active: bool,
) {
	repository.remote = true;
	repository.active = active;
	agent.allow_task_creation = Some(false);
	assert!(matches!(
		authorize(&repository, &run, &agent, &call("task_create", json!({}))).await,
		Err(Error::Forbidden)
	));
	let mut expected = vec!["lease", "active"];
	if active {
		expected.push("suspend");
	}
	expected.extend(["replace", "release"]);
	assert_eq!(repository.calls(), expected);
	assert!(repository.decisions().is_empty());
}
#[rstest]
#[tokio::test]
async fn local_refresh_has_no_authority_io(repository: Repository) {
	refresh(&repository).await.unwrap();
	assert!(repository.calls().is_empty());
}
#[rstest]
#[tokio::test]
async fn missing_remote_lease_denies_before_tool_authorization(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.remote = true;
	repository.replace = false;
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call("workspace_read", json!({}))
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec!["lease", "active", "suspend", "replace", "release"]
	);
	assert!(repository.decisions().is_empty());
}
#[rstest]
#[case::suspend("suspend",vec!["lease","active","suspend","release"])]
#[case::replace("replace",vec!["lease","active","suspend","replace","release"])]
#[tokio::test]
async fn refresh_failures_release_authority_and_keep_the_adapter_fault(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] fault: &'static str,
	#[case] expected: Vec<&str>,
) {
	repository.remote = true;
	repository.fault = Some(fault);
	assert_fault(
		authorize(
			&repository,
			&run,
			&agent,
			&call("workspace_read", json!({})),
		)
		.await
		.err()
		.unwrap(),
		&format!("{fault} fault"),
	);
	assert_eq!(repository.calls(), expected);
}
#[rstest]
#[tokio::test]
async fn remote_tool_evaluation_reacquires_after_releasing_its_refresh_lease(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.remote = true;
	authorize(
		&repository,
		&run,
		&agent,
		&call("workspace_read", json!({})),
	)
	.await
	.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease",
			"active",
			"suspend",
			"replace",
			"release",
			"lease",
			"require:builtin:workspace_read:tool.invoke",
			"release"
		]
	);
}
#[rstest]
#[tokio::test]
async fn unknown_builtin_is_denied_after_its_invoke_decision(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	assert!(matches!(
		authorize(&repository, &run, &agent, &call("unknown", json!({}))).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls(), Vec::<&str>::new());
}
#[rstest]
#[tokio::test]
async fn native_plugin_requires_the_pinned_catalog_without_a_builtin_decision(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	authorize(&repository, &run, &agent, &call("plugin_0", json!({})))
		.await
		.unwrap();
	assert_eq!(
		repository.calls(),
		vec!["lease", "catalog:configured:tool.invoke", "release"]
	);
	assert!(repository.decisions().is_empty());
}
#[rstest]
#[case::missing_index("plugin_9",vec![])]
#[case::malformed_index("plugin_bad",vec![])]
#[tokio::test]
async fn unavailable_aliases_are_rejected_before_tool_authorization(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] name: &str,
	#[case] expected: Vec<&str>,
) {
	assert!(matches!(
		authorize(&repository, &run, &agent, &call(name, json!({}))).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls(), expected);
}
#[rstest]
#[case::disabled(true)]
#[case::foreign_target(false)]
#[tokio::test]
async fn agent_plugins_check_delegation_flag_and_home_before_target_catalog(
	mut repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] disabled: bool,
) {
	repository.catalog.config = json!({"transport":"agent","node_id":if disabled{run.home_node.as_str()}else{"aidash://other"},"agent":{"id":"target","version":"1"}});
	if disabled {
		agent.allow_task_delegation = Some(false);
	}
	assert!(matches!(
		authorize(&repository, &run, &agent, &call("plugin_0", json!({}))).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec!["lease", "catalog:configured:tool.invoke", "release"]
	);
}
#[rstest]
#[tokio::test]
async fn an_agent_plugin_requires_target_execution_and_workspace_creation(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.catalog.config =
		json!({"transport":"agent","node_id":run.home_node,"agent":{"id":"target","version":"1"}});
	authorize(&repository, &run, &agent, &call("plugin_0", json!({})))
		.await
		.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease".into(),
			"catalog:configured:tool.invoke".into(),
			"catalog:target:agent.execute".into(),
			format!("require:{}:task.create", run.workspace_id),
			"release".into()
		]
	);
	let decisions = repository.decisions();
	assert_eq!(decisions[0].0.attributes, json!({}));
	assert_eq!(decisions[0].0.kind, "workspace");
}
#[rstest]
#[tokio::test]
async fn malformed_plugin_configuration_keeps_the_json_error(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.catalog.config = json!({"transport":"unsupported"});
	assert!(matches!(
		authorize(&repository, &run, &agent, &call("plugin_0", json!({}))).await,
		Err(Error::Json(_))
	));
	assert_eq!(
		repository.calls(),
		vec!["lease", "catalog:configured:tool.invoke", "release"]
	);
}
#[rstest]
#[case::files("file_read")]
#[case::shell("shell_poll")]
#[case::python("python_install")]
#[case::patch("apply_patch")]
#[case::sharing("file_share")]
#[case::outbound("outbound_get")]
#[tokio::test]
async fn core_tools_require_invoke_and_the_declared_capability(
	repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] name: &str,
) {
	agent.core_capabilities.files = true;
	agent.core_capabilities.shell = true;
	agent.core_capabilities.python = true;
	agent.core_capabilities.patch = true;
	agent.core_capabilities.sharing = true;
	authorize(&repository, &run, &agent, &call(name, json!({})))
		.await
		.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease".into(),
			format!("require:builtin:{name}:tool.invoke"),
			"release".into()
		]
	);
}
#[rstest]
#[tokio::test]
async fn missing_core_capability_is_denied_after_the_invoke_check(
	repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	agent.core_capabilities.files = false;
	assert!(matches!(
		authorize(&repository, &run, &agent, &call("file_read", json!({}))).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec!["lease", "require:builtin:file_read:tool.invoke", "release"]
	);
}
#[rstest]
#[case::list("skill_list",json!({}))]
#[case::load("skill_load",json!({}))]
#[case::read_id("skill_read",json!({"skill_id":null}))]
#[tokio::test]
async fn core_skill_operations_use_the_skill_flag_even_for_a_null_skill_id(
	repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] name: &str,
	#[case] arguments: Value,
) {
	agent.core_capabilities.skills = true;
	authorize(&repository, &run, &agent, &call(name, arguments))
		.await
		.unwrap();
	assert!(
		repository
			.calls()
			.iter()
			.all(|name| !name.starts_with("catalog:"))
	);
	assert_eq!(repository.decisions().len(), 1);
}
#[rstest]
#[case::task("task_create", "workspace", "task.create")]
#[case::message("workspace_message", "workspace", "message.create")]
#[case::human("human_request", "run", "human.request")]
#[case::generation("task_assign", "generation_policy", "generation.request")]
#[case::memory("memory_mutate", "memory", "memory.write")]
#[tokio::test]
async fn protected_builtins_use_the_saved_resource_and_exact_action(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] name: &str,
	#[case] kind: &str,
	#[case] action: &str,
) {
	authorize(
		&repository,
		&run,
		&agent,
		&call(name, json!({"policy_id":"saved-policy"})),
	)
	.await
	.unwrap();
	let decisions = repository.decisions();
	assert_eq!(decisions.len(), 2);
	assert_eq!(decisions[0].1, "tool.invoke");
	assert_eq!(decisions[1].1, action);
	assert_eq!(decisions[1].0.kind, kind);
	let expected = match kind {
		"workspace" => run.workspace_id.to_string(),
		"run" => run.id.to_string(),
		"generation_policy" => "saved-policy".into(),
		"memory" => run.agent_id.clone(),
		_ => panic!("unexpected fixture"),
	};
	assert_eq!(decisions[1].0.id, expected);
	if kind == "memory" {
		assert_eq!(
			decisions[1].0.attributes,
			json!({"workspace":run.workspace_id,"run":run.id})
		);
	} else {
		assert_eq!(decisions[1].0.attributes, json!({}));
	}
}
#[rstest]
#[tokio::test]
async fn task_delegation_checks_target_catalog_before_saved_task_visibility(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	authorize(
		&repository,
		&run,
		&agent,
		&call(
			"task_delegate",
			json!({"task_id":id(9),"node_id":run.home_node,"agent":reference()}),
		),
	)
	.await
	.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease".into(),
			"require:builtin:task_delegate:tool.invoke".into(),
			"catalog:configured:agent.execute".into(),
			format!("task_read:{}", id(9)),
			"task_resource".into(),
			format!("require:{}:task.delegate", id(9)),
			"release".into()
		]
	);
	assert_eq!(
		repository.decisions()[1].0.attributes,
		json!({"revision":7,"creator":"saved creator"})
	);
}
#[rstest]
#[case::missing_policy("task_assign",json!({}),"missing generation policy")]
#[case::invalid_task("task_delegate",json!({"task_id":"invalid"}),"invalid task id")]
#[tokio::test]
async fn invalid_builtin_arguments_keep_the_existing_error_after_invoke(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] name: &str,
	#[case] arguments: Value,
	#[case] message: &str,
) {
	assert!(
		matches!(authorize(&repository,&run,&agent,&call(name,arguments)).await,Err(Error::Invalid(ref actual)) if actual==message)
	);
	assert_eq!(repository.decisions().len(), 1);
}
#[rstest]
#[tokio::test]
async fn a_foreign_delegate_target_is_denied_before_catalog_io(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call(
				"task_delegate",
				json!({"task_id":id(9),"node_id":"aidash://other","agent":reference()})
			)
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec![
			"lease",
			"require:builtin:task_delegate:tool.invoke",
			"release"
		]
	);
}
#[rstest]
#[tokio::test]
async fn malformed_delegate_agent_keeps_the_json_error(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call(
				"task_delegate",
				json!({"task_id":id(9),"node_id":run.home_node,"agent":null})
			)
		)
		.await,
		Err(Error::Json(_))
	));
	assert_eq!(repository.decisions().len(), 1);
}
#[rstest]
#[tokio::test]
async fn artifact_creation_uses_the_last_subject_in_the_delegation_chain(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	authorize(
		&repository,
		&run,
		&agent,
		&call("artifact_publish", json!({})),
	)
	.await
	.unwrap();
	assert_eq!(
		repository.decisions()[1].0.attributes,
		json!({"created_by":"delegated"})
	);
	assert!(repository.calls().contains(&"artifact:delegated".into()));
}
#[rstest]
#[tokio::test]
async fn local_artifact_creation_requires_a_nonempty_subject_chain(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.subjects.clear();
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call("artifact_publish", json!({}))
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.decisions().len(), 1);
}
#[rstest]
#[tokio::test]
async fn remote_artifact_authority_uses_qualified_id_and_current_context(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.remote = true;
	repository.subjects.clear();
	authorize(
		&repository,
		&run,
		&agent,
		&call("artifact_publish", json!({})),
	)
	.await
	.unwrap();
	let decision = &repository.decisions()[1];
	assert_eq!(
		decision.0.id,
		format!("aidash://home/artifacts/{}", run.task_id)
	);
	assert_eq!(decision.0.attributes, repository.context);
	assert!(
		!repository
			.calls()
			.iter()
			.any(|name| name.starts_with("artifact:"))
	);
}
#[rstest]
#[tokio::test]
async fn a_remote_delegate_cannot_address_a_task_other_than_its_admitted_task(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.remote = true;
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call(
				"task_delegate",
				json!({"task_id":id(9),"node_id":run.home_node,"agent":reference()})
			)
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(
		repository
			.calls()
			.contains(&"catalog:configured:agent.execute".into())
	);
	assert!(
		!repository
			.calls()
			.iter()
			.any(|name| name.starts_with("task_read:"))
	);
	assert_eq!(repository.decisions().len(), 1);
}
#[rstest]
#[case::memory("memory_mutate")]
#[case::generation("task_assign")]
#[tokio::test]
async fn remote_execution_rejects_local_only_resources_after_invoke(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] name: &str,
) {
	repository.remote = true;
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call(name, json!({"policy_id":"saved"}))
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.decisions().len(), 1);
}
#[rstest]
#[tokio::test]
async fn a_configured_registry_skill_uses_catalog_authority_without_a_core_flag(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.catalog.kind = "skill".into();
	authorize(
		&repository,
		&run,
		&agent,
		&call("skill_read", json!({"skill":reference()})),
	)
	.await
	.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease",
			"require:builtin:skill_read:tool.invoke",
			"catalog:configured:skill.use",
			"release"
		]
	);
}
#[rstest]
#[case::not_configured(true)]
#[case::wrong_kind(false)]
#[tokio::test]
async fn registry_skill_requires_both_configuration_and_a_skill_entry(
	mut repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] not_configured: bool,
) {
	if not_configured {
		agent.skills.clear();
	} else {
		repository.catalog.kind = "tool".into();
	}
	assert!(matches!(
		authorize(
			&repository,
			&run,
			&agent,
			&call("skill_read", json!({"skill":reference()}))
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository
			.calls()
			.iter()
			.filter(|name| name.starts_with("catalog:"))
			.count(),
		usize::from(!not_configured)
	);
}
#[rstest]
#[tokio::test]
async fn malformed_registry_skill_keeps_the_original_invalid_json_message(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	let expected = serde_json::from_value::<EntityRef>(Value::Null)
		.unwrap_err()
		.to_string();
	assert!(
		matches!(authorize(&repository,&run,&agent,&call("skill_read",json!({"skill":null}))).await,Err(Error::Invalid(ref message)) if message==&expected)
	);
}
/// A deferred snapshot binding the configured Registry Skill; returns its alias.
fn asset_snapshot(repository: &mut Repository, run: &RunMetadata) -> String {
	let node = run.home_node.as_str();
	let skill = QualifiedRef {
		registry_node: node.into(),
		id: reference().id,
		version: reference().version,
	};
	let agent = QualifiedRef {
		registry_node: node.into(),
		id: run.agent_id.clone(),
		version: run.agent_version.clone(),
	};
	let definition = |identity: &QualifiedRef, kind: &str, config: Value| -> Entry {
		serde_json::from_value(json!({
			"id": identity.id, "version": identity.version, "kind": kind,
			"name": {"en": identity.id}, "description": {"en": "Fixture"}, "config": config,
		}))
		.unwrap()
	};
	let skill_entry = definition(&skill, "skill", json!({"instructions":"Use it."}));
	let agent_entry = definition(
		&agent,
		"agent",
		json!({"schema_version":1,"model":{"id":"model","version":"1"},"instructions":"Work.","bindings":[],"exposure":{"version":"deferred@1"}}),
	);
	let snapshot = BindingSnapshot {
		schema_version: 1,
		agent: agent.clone(),
		remote: false,
		bindings: vec![ResolvedBinding {
			identity: skill.clone(),
			definition: skill_entry.clone(),
			digest: "skill-digest".into(),
			origin: BindingOrigin::Explicit,
			alias: None,
			narrow: Narrowing::default(),
			installation: None,
			provider_contract_digest: None,
			provider_implementation: None,
			excluded_reason: None,
		}],
		definitions: [(skill, skill_entry), (agent, agent_entry)]
			.into_iter()
			.map(|(identity, definition)| ResolvedDefinition {
				identity,
				definition,
				digest: "digest".into(),
			})
			.collect(),
		foreign_agents: vec![],
	};
	let alias = aidash_domain::exposure::catalog(&snapshot, &BTreeMap::new(), &[])
		.unwrap()
		.remove(0)
		.alias;
	repository.snapshot = Some(snapshot);
	alias
}
#[rstest]
#[tokio::test]
async fn a_registry_skill_asset_read_uses_catalog_authority_without_a_core_flag(
	mut repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
) {
	let alias = asset_snapshot(&mut repository, &run);
	repository.catalog.kind = "skill".into();
	agent.core_capabilities.skills = false;
	let read = call(
		"skill_asset_read",
		json!({"alias":alias,"digest":"d","path":"a"}),
	);
	authorize(&repository, &run, &agent, &read).await.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease",
			"require:builtin:skill_asset_read:tool.invoke",
			"catalog:configured:skill.use",
			"release"
		]
	);
	agent.skills.clear();
	assert!(matches!(
		authorize(&repository, &run, &agent, &read).await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case::skills_off(false)]
#[case::skills_on(true)]
#[tokio::test]
async fn a_direct_skill_asset_read_requires_the_core_skills_permission(
	mut repository: Repository,
	run: RunMetadata,
	mut agent: AgentConfig,
	#[case] skills: bool,
) {
	asset_snapshot(&mut repository, &run);
	agent.core_capabilities.skills = skills;
	let result = authorize(
		&repository,
		&run,
		&agent,
		&call(
			"skill_asset_read",
			json!({"alias":"skill_direct_0000","digest":"d","path":"a"}),
		),
	)
	.await;
	assert_eq!(result.is_ok(), skills);
	assert!(
		!skills
			|| !repository
				.calls()
				.iter()
				.any(|call| call.starts_with("catalog:"))
	);
	if !skills {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
}
#[rstest]
#[case::discovery("agent_discover")]
#[case::observe("workspace_observe")]
#[case::read("workspace_read")]
#[case::wait("workspace_wait")]
#[tokio::test]
async fn discovery_and_workspace_retrieval_leave_resource_checks_to_their_use_case(
	repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
	#[case] name: &str,
) {
	authorize(&repository, &run, &agent, &call(name, json!({})))
		.await
		.unwrap();
	assert_eq!(repository.decisions().len(), 1);
	assert_eq!(repository.decisions()[0].1, "tool.invoke");
}
#[rstest]
#[tokio::test]
async fn filtering_removes_denied_tools_immediately_before_a_later_adapter_fault(
	mut repository: Repository,
	mut agent: AgentConfig,
) {
	// Arrange: sorted names expose two denials before a later failure.
	agent.core_capabilities.files = true;
	agent.core_capabilities.python = true;
	let mut tools = BTreeMap::from([
		("apply_patch".into(), 1),
		("file_read".into(), 2),
		("file_search".into(), 3),
		("python_install".into(), 4),
		("python_poll".into(), 5),
	]);
	for (name, outcome) in [
		("file_read", Outcome::Forbidden),
		("file_search", Outcome::Missing),
		("python_install", Outcome::Fault),
	] {
		repository
			.outcomes
			.insert((format!("builtin:{name}"), "tool.invoke".into()), outcome);
	}
	// Act: a failure must preserve removals already applied by the use case.
	assert_fault(
		filter(&repository, &agent, &mut tools).await.err().unwrap(),
		"require fault",
	);
	// Assert: non-enabled capabilities and later tools retain their original values.
	assert_eq!(
		tools,
		BTreeMap::from([
			("apply_patch".into(), 1),
			("python_install".into(), 4),
			("python_poll".into(), 5)
		])
	);
	assert_eq!(
		repository.calls(),
		vec![
			"lease",
			"require:builtin:file_read:tool.invoke",
			"require:builtin:file_search:tool.invoke",
			"require:builtin:python_install:tool.invoke",
			"release"
		]
	);
}
#[rstest]
#[tokio::test]
async fn remote_filtering_happens_before_lease_failure_and_does_not_refresh(
	mut repository: Repository,
	agent: AgentConfig,
) {
	repository.remote = true;
	repository.fault = Some("lease");
	let mut tools = BTreeMap::from([
		("memory_mutate".into(), 1),
		("task_create".into(), 2),
		("task_delegate".into(), 3),
		("plugin_bad".into(), 4),
		("skill_read".into(), 5),
		("human_request".into(), 6),
	]);
	assert_fault(
		filter(&repository, &agent, &mut tools).await.err().unwrap(),
		"lease fault",
	);
	assert_eq!(
		tools,
		BTreeMap::from([
			("task_create".into(), 2),
			("plugin_bad".into(), 4),
			("skill_read".into(), 5)
		])
	);
	assert_eq!(repository.calls(), vec!["lease"]);
}
#[rstest]
#[tokio::test]
async fn cancellation_releases_the_lease_held_during_tool_authorization(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.stall = Some("tool.invoke".into());
	let input = call("workspace_read", json!({}));
	let mut pending = Box::pin(authorize(&repository, &run, &agent, &input));
	assert!(futures_util::poll!(pending.as_mut()).is_pending());
	assert_eq!(
		repository.calls(),
		vec!["lease", "require:builtin:workspace_read:tool.invoke"]
	);
	drop(pending);
	assert_eq!(repository.calls().last().unwrap(), "release");
}
#[rstest]
#[case::matching_task("task", 2, true)]
#[case::foreign_task("task", 9, false)]
#[case::matching_workspace("workspace", 3, true)]
#[case::foreign_workspace("workspace", 9, false)]
#[case::memory("memory", 1, false)]
#[case::run("run", 1, true)]
#[case::foreign_run("run", 9, false)]
#[case::generation("generation_policy", 1, false)]
fn remote_identifiers_enforce_the_admitted_scope(
	run: RunMetadata,
	#[case] kind: &str,
	#[case] key: u128,
	#[case] allowed: bool,
) {
	let result = remote_identifier(&run, kind, &id(key).to_string());
	if allowed {
		assert_eq!(
			result.unwrap(),
			format!("aidash://home/{kind}s/{}", id(key))
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
}

#[rstest]
#[tokio::test]
async fn explicit_actions_require_saved_task_visibility_before_the_final_action(
	repository: Repository,
	run: RunMetadata,
) {
	action(&repository, &run, "task.update", "task", &id(9).to_string())
		.await
		.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease".into(),
			format!("task_read:{}", id(9)),
			"task_resource".into(),
			format!("require:{}:task.update", id(9)),
			"release".into()
		]
	);
	assert_eq!(
		repository.decisions()[0].0.attributes,
		json!({"revision":7,"creator":"saved creator"})
	);
}
#[rstest]
#[tokio::test]
async fn invalid_explicit_task_id_is_forbidden_before_reading_a_task(
	repository: Repository,
	run: RunMetadata,
) {
	assert!(matches!(
		action(&repository, &run, "task.update", "task", "invalid").await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.calls(), vec!["lease", "release"]);
}
#[rstest]
#[tokio::test]
async fn explicit_remote_workspace_actions_refresh_and_keep_current_context(
	mut repository: Repository,
	run: RunMetadata,
) {
	repository.remote = true;
	action(
		&repository,
		&run,
		"workspace.read",
		"workspace",
		&run.workspace_id.to_string(),
	)
	.await
	.unwrap();
	let decision = &repository.decisions()[0];
	assert_eq!(
		decision.0.id,
		format!("aidash://home/workspaces/{}", run.workspace_id)
	);
	assert_eq!(decision.0.attributes, repository.context);
	assert_eq!(decision.1, "workspace.read");
	assert_eq!(
		repository.calls()[..5],
		vec!["lease", "active", "suspend", "replace", "release"]
	);
}
#[rstest]
#[case::workspace("workspace")]
#[case::task("task")]
#[case::memory("memory")]
#[case::run("run")]
#[case::generation("generation_policy")]
#[tokio::test]
async fn explicit_remote_actions_cannot_escape_the_admitted_resource(
	mut repository: Repository,
	run: RunMetadata,
	#[case] kind: &str,
) {
	repository.remote = true;
	assert!(matches!(
		action(&repository, &run, "read", kind, &id(9).to_string()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repository.calls(),
		vec![
			"lease", "active", "suspend", "replace", "release", "lease", "release"
		]
	);
	assert!(repository.decisions().is_empty());
}
#[rstest]
#[tokio::test]
async fn remote_human_reads_are_denied_before_acquiring_or_refreshing_authority(
	mut repository: Repository,
	run: RunMetadata,
) {
	repository.remote = true;
	assert!(matches!(
		human_read(&repository, &run, id(9)).await,
		Err(Error::Forbidden)
	));
	assert!(repository.calls().is_empty());
}
#[rstest]
#[tokio::test]
async fn a_saved_human_request_is_authorized_under_its_run_and_workspace(
	repository: Repository,
	run: RunMetadata,
) {
	human_read(&repository, &run, id(9)).await.unwrap();
	assert_eq!(
		repository.calls(),
		vec![
			"lease".into(),
			format!("human:{}:{}:{}", run.id, run.workspace_id, id(9)),
			"human_resource".into(),
			format!("require:{}:human.read", id(9)),
			"release".into()
		]
	);
	let decision = &repository.decisions()[0];
	assert_eq!(decision.0.kind, "human_request");
	assert_eq!(
		decision.0.attributes,
		json!({"run":run.id,"workspace":run.workspace_id,"prompt":"saved question"})
	);
	assert_eq!(decision.1, "human.read");
}
#[rstest]
#[tokio::test]
async fn a_missing_scoped_request_is_forbidden_without_constructing_a_resource(
	mut repository: Repository,
	run: RunMetadata,
) {
	repository.missing_request = true;
	assert!(matches!(
		human_read(&repository, &run, id(9)).await,
		Err(Error::Forbidden)
	));
	assert!(repository.decisions().is_empty());
	assert!(!repository.calls().contains(&"human_resource".into()));
	assert_eq!(repository.calls().last().unwrap(), "release");
}
#[rstest]
#[tokio::test]
async fn human_request_denial_releases_authority_after_the_exact_read_decision(
	mut repository: Repository,
	run: RunMetadata,
) {
	repository
		.outcomes
		.insert((id(9).to_string(), "human.read".into()), Outcome::Forbidden);
	assert!(matches!(
		human_read(&repository, &run, id(9)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.decisions()[0].1, "human.read");
	assert_eq!(repository.calls().last().unwrap(), "release");
}
#[rstest]
#[case::query("human")]
#[case::resource("human_resource")]
#[tokio::test]
async fn human_read_adapter_failures_keep_their_identity_without_a_final_decision(
	mut repository: Repository,
	run: RunMetadata,
	#[case] stage: &'static str,
) {
	repository.fault = Some(stage);
	assert_fault(
		human_read(&repository, &run, id(9)).await.err().unwrap(),
		&format!("{stage} fault"),
	);
	assert!(repository.decisions().is_empty());
	assert_eq!(repository.calls().last().unwrap(), "release");
}
#[rstest]
#[tokio::test]
async fn cancelling_a_human_read_releases_the_held_authority(
	mut repository: Repository,
	run: RunMetadata,
) {
	repository.stall = Some("human.read".into());
	let mut pending = Box::pin(human_read(&repository, &run, id(9)));
	assert!(futures_util::poll!(pending.as_mut()).is_pending());
	assert_ne!(repository.calls().last().unwrap(), "release");
	drop(pending);
	assert_eq!(repository.calls().last().unwrap(), "release");
}

#[rstest]
#[tokio::test]
async fn remote_human_request_authorizes_the_exact_home_run(
	mut repository: Repository,
	run: RunMetadata,
	agent: AgentConfig,
) {
	repository.remote = true;
	authorize(
		&repository,
		&run,
		&agent,
		&call(
			"human_request",
			json!({"kind":"question","prompt":"Continue?"}),
		),
	)
	.await
	.unwrap();
	assert!(repository.calls().contains(&format!(
		"require:aidash://home/runs/{}:human.request",
		run.id
	)));
}
