//! Tool orchestration tests use portable identities and recording effect ports.
use super::*;
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, ReadyState, RecoveryState, Run, RunControl,
	RunState, StateVersion, Task, TaskStatus, context::Context, registry::SkillFile,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::Mutex;
use uuid::Uuid;
struct Operations {
	task: Task,
	skills: Vec<EntityRef>,
	skill_text: String,
	calls: Mutex<Vec<&'static str>>,
	inputs: Mutex<Vec<(String, Value)>>,
}
impl Operations {
	fn record(&self, name: &'static str) {
		self.calls.lock().unwrap().push(name);
	}
}
#[async_trait]
impl ToolOperations for Operations {
	async fn registered_skills(&self) -> Result<Vec<EntityRef>> {
		self.record("skills.authorize");
		Ok(self.skills.clone())
	}
	async fn skill_files(&self, reference: &EntityRef) -> Result<Vec<SkillFile>> {
		self.record("skills.read");
		assert!(self.skills.contains(reference));
		Ok(vec![SkillFile {
			path: "SKILL.md".into(),
			content: self.skill_text.clone(),
			encoding: None,
		}])
	}
	async fn discover(&self, _search: &Search) -> Result<Value> {
		panic!("unexpected tool effect: discover")
	}
	async fn create_task(&self, key: &str, input: &NewTask) -> Result<Task> {
		self.record("task.create");
		self.inputs
			.lock()
			.unwrap()
			.push((key.to_owned(), serde_json::to_value(input)?));
		Ok(self.task.clone())
	}
	async fn assign(&self, _id: Uuid, _policy: &str, _reason: &str) -> Result<Value> {
		panic!("unexpected tool effect: assign")
	}
	async fn delegate_with_key(
		&self,
		key: &str,
		id: Uuid,
		node: &str,
		agent: &EntityRef,
	) -> Result<Value> {
		self.record("task.delegate");
		assert_eq!(id, self.task.id);
		self.inputs
			.lock()
			.unwrap()
			.push((key.to_owned(), json!({"node":node,"agent":agent})));
		Ok(json!({"delivered":true}))
	}
	async fn artifact(&self, _key: &str, _input: &ArtifactInput) -> Result<Artifact> {
		panic!("unexpected tool effect: artifact")
	}
	async fn message(&self, _key: &str, _content: &str) -> Result<()> {
		panic!("unexpected tool effect: message")
	}
	async fn observation(&self, _offset: usize, _limit: usize) -> Result<Value> {
		panic!("unexpected tool effect: observation")
	}
	async fn read_record_chunk(
		&self,
		_kind: &str,
		_id: &str,
		_offset: usize,
		_maximum: usize,
	) -> Result<Value> {
		panic!("unexpected tool effect: read_record_chunk")
	}
	async fn memory_mutate(
		&self,
		_key: &str,
		_changes: &[aidash_domain::memory::Change],
	) -> Result<Vec<aidash_domain::memory::Unit>> {
		panic!("unexpected native mutation");
	}
	async fn memory_recall(
		&self,
		_key: &str,
		_query: &aidash_domain::memory::RecallQuery,
		_reflect: bool,
	) -> Result<Value> {
		panic!("unexpected native recall");
	}
	async fn human_request(&self, _kind: &str, _prompt: &str, _key: &str) -> Result<HumanRequest> {
		panic!("unexpected tool effect: human_request")
	}
}
#[async_trait]
impl ToolTransport for Operations {
	async fn invoke(&self, _config: &ToolConfig, input: Value, key: &str) -> Result<Value> {
		self.record("transport.invoke");
		self.inputs
			.lock()
			.unwrap()
			.push((key.to_owned(), input.clone()));
		Ok(input)
	}
}
struct Fixture {
	operations: Operations,
	run: Run,
}
#[fixture]
fn fixture() -> Fixture {
	let now = "2026-10-02T00:00:00Z".parse().unwrap();
	let token = Uuid::new_v4();
	let workspace = Uuid::new_v4();
	let task = Uuid::new_v4();
	let run = Run {
		id: Uuid::new_v4(),
		task_id: task,
		workspace_id: workspace,
		home_node: "aidash://fixture".into(),
		agent_id: "agent".into(),
		agent_version: "1.0.0".into(),
		state_version: StateVersion::default(),
		state: RunState::Ready(ReadyState::default()),
		recovery: RecoveryState::default(),
		control: RunControl::Active,
		context: Context::default(),
		step: 0,
		revision: 0,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: Some(token),
		lease_until: Some(now),
		updated_at: now,
	};

	let task = Task {
		id: Uuid::new_v4(),
		workspace_id: workspace,
		title: "Child".into(),
		description: "Child task".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "fixture".into(),
		dependencies: vec![],
		parent_id: Some(run.task_id),
		revision: 0,
		created_at: now,
	};
	Fixture {
		operations: Operations {
			skill_text: "界ab".into(),
			task,
			skills: vec![],
			calls: Mutex::new(vec![]),
			inputs: Mutex::new(vec![]),
		},
		run,
	}
}
fn plugin(config: ToolConfig, schema: Value) -> Plugin {
	let entry: Entry = serde_json::from_value(json!({"id":"fixture-tool","version":"1.0.0","kind":"tool","name":{"en":"Fixture"},"description":{"en":"Fixture"},"config":{},"schema":schema})).unwrap();
	Plugin {
		entry,
		alias: "plugin_0".into(),
		config,
	}
}
#[rstest]
#[case("http://other.invalid/path")]
#[case("ftp://allowed.invalid/path")]
#[case("https://user:password@allowed.invalid/path")]
#[tokio::test]
async fn denied_native_urls_never_reach_the_transport(fixture: Fixture, #[case] url: &str) {
	// Arrange
	let tool = plugin(
		ToolConfig::Native {
			operation: "http_get".into(),
			allowed_hosts: vec!["allowed.invalid".into()],
		},
		json!({"type":"object"}),
	);
	// Act
	let result = tool
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			&fixture.operations,
			json!({"url":url}),
			"key",
		)
		.await;
	// Assert
	assert!(matches!(result, Err(Error::Invalid(_))));
	assert!(fixture.operations.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn schema_rejection_precedes_every_tool_effect(fixture: Fixture) {
	// Arrange
	let tool = plugin(
		ToolConfig::Http {
			endpoint: "http://fixture.invalid".into(),
			credential_env: None,
			replay: "idempotent".into(),
		},
		json!({"type":"object","required":["name"]}),
	);
	// Act
	let result = tool
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			&fixture.operations,
			json!({}),
			"key",
		)
		.await;
	// Assert
	assert!(matches!(result, Err(Error::Invalid(_))));
	assert!(fixture.operations.calls.lock().unwrap().is_empty());
}
#[rstest]
#[tokio::test]
async fn agent_delegation_retains_parent_and_durable_operation_keys(fixture: Fixture) {
	// Arrange
	let agent = EntityRef {
		id: "agent".into(),
		version: "1.0.0".into(),
	};
	let tool = plugin(
		ToolConfig::Agent {
			node_id: "aidash://remote".into(),
			agent: agent.clone(),
		},
		json!({"type":"object"}),
	);
	// Act
	let output = tool
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			&fixture.operations,
			json!({"title":"Child","description":"Do the child task"}),
			"invocation-1",
		)
		.await
		.unwrap();
	// Assert
	assert_eq!(
		*fixture.operations.calls.lock().unwrap(),
		["task.create", "task.delegate"]
	);
	let inputs = fixture.operations.inputs.lock().unwrap();
	assert_eq!(inputs[0].0, "invocation-1:task");
	assert_eq!(inputs[0].1["parent_id"], json!(fixture.run.task_id));
	assert_eq!(
		inputs[1],
		(
			"invocation-1:delegate".into(),
			json!({"node":"aidash://remote","agent":agent})
		)
	);
	assert_eq!(
		output,
		json!({"task":fixture.operations.task,"delegation":{"delivered":true}})
	);
}
#[rstest]
#[tokio::test]
async fn unregistered_skill_is_rejected_before_its_contents_are_read(fixture: Fixture) {
	// Arrange
	let tool = builtins().remove("skill_read").unwrap();
	// Act
	let result = tool
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			json!({"skill":{"id":"skill","version":"1.0.0"},"path":"SKILL.md"}),
			"read-key",
		)
		.await;
	// Assert
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(
		*fixture.operations.calls.lock().unwrap(),
		["skills.authorize"]
	);
}
#[rstest]
#[tokio::test]
async fn authorized_skill_chunks_use_scalar_offsets(mut fixture: Fixture) {
	// Arrange
	fixture.operations.skills.push(EntityRef {
		id: "skill".into(),
		version: "1.0.0".into(),
	});
	let tool = builtins().remove("skill_read").unwrap();
	// Act
	let output = tool
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			json!({"skill":{"id":"skill","version":"1.0.0"},"path":"SKILL.md","offset":1,"max_chars":1}),
			"read-key",
		)
		.await
		.unwrap();
	// Assert
	assert_eq!(
		*fixture.operations.calls.lock().unwrap(),
		["skills.authorize", "skills.read"]
	);
	assert_eq!(
		output,
		json!({"path":"SKILL.md","text":"a","encoding":"utf8","offset":1,"total_chars":3,"next_offset":2})
	);
}

#[rstest]
#[case("abc")]
#[case("日本語")]
#[case("A😀B")]
#[case("e\u{301}界")]
#[tokio::test]
async fn legacy_skill_pages_reconstruct_with_scalar_positions(
	mut fixture: Fixture,
	#[case] text: &str,
) {
	let reference = EntityRef {
		id: "skill".into(),
		version: "1.0.0".into(),
	};
	fixture.operations.skills.push(reference.clone());
	fixture.operations.skill_text = text.into();
	let tool = builtins().remove("skill_read").unwrap();
	let mut offset = 0;
	let mut combined = String::new();
	loop {
		let output = tool
			.invoke(
				&ToolContext {
					operations: &fixture.operations,
					run: &fixture.run,
				},
				json!({"skill":reference,"path":"SKILL.md","offset":offset,"max_chars":1}),
				"read",
			)
			.await
			.unwrap();
		assert_eq!(output["offset"], offset);
		assert_eq!(output["total_chars"], text.chars().count());
		let chunk = output["text"].as_str().unwrap();
		assert_eq!(chunk.chars().count(), 1);
		combined.push_str(chunk);
		let Some(next) = output["next_offset"].as_u64() else {
			break;
		};
		assert_eq!(next as usize, offset + 1);
		offset = next as usize;
	}
	assert_eq!(combined, text);
}

#[rstest::rstest]
#[case("add", None)]
#[case("correct", Some(1))]
#[case("delete", Some(1))]
fn native_memory_tool_schema_resolves_nested_content_at_the_root(
	#[case] operation: &str,
	#[case] revision: Option<i64>,
) {
	let mut change = json!({"operation":operation,"id":uuid::Uuid::new_v4()});
	if let Some(revision) = revision {
		change["expected_revision"] = json!(revision);
	}
	if operation != "delete" {
		change["content"] = json!({"text":"東京 Tokyo", "kind":"world","learning":"fact","verification":"unverified","occurred":null,"entities":[],"evidence":[],"links":[]});
	}
	let schema = &super::builtins()["memory_mutate"].schema;
	super::validate_arguments(schema, &json!({"changes":[change]})).unwrap();
}

#[rstest]
#[case("add", "supported")]
#[case("add", "contradicted")]
#[case("correct", "supported")]
#[case("correct", "contradicted")]
#[tokio::test]
async fn native_memory_model_tool_cannot_attest_claim_verification(
	fixture: Fixture,
	#[case] operation: &str,
	#[case] verification: &str,
) {
	let mut change = json!({"operation":operation,"id":Uuid::new_v4(),"content":{"text":"Unverified model claim / 未検証のモデル出力","kind":"world","learning":"fact","verification":verification,"occurred":null,"entities":[],"evidence":[],"links":[]}});
	if operation == "correct" {
		change["expected_revision"] = json!(1);
	}
	let result = super::builtins()["memory_mutate"]
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			json!({"changes":[change]}),
			"model-verification",
		)
		.await;
	assert!(matches!(result, Err(Error::Invalid(_))));
	assert!(
		fixture.operations.calls.lock().unwrap().is_empty(),
		"reject before canonical mutation or admission"
	);
}
