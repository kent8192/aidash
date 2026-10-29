//! Tool orchestration tests use portable identities and recording effect ports.
use super::*;
use crate::tenant_secrets::{CallSecret, SecretRecord};
use aidash_domain::{
	Artifact, ArtifactInput, HumanRequest, NewTask, ReadyState, RecoveryState, Run, RunControl,
	RunState, StateVersion, Task, TaskStatus, context::Context, registry::SkillFile,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::sync::Mutex;
use uuid::Uuid;
struct Operations {
	proxy: Option<Value>,
	task: Task,
	skills: Vec<EntityRef>,
	skill_text: String,
	calls: Mutex<Vec<&'static str>>,
	inputs: Mutex<Vec<(String, Value)>>,
	connection: bool,
	person: bool,
}
impl Operations {
	fn record(&self, name: &'static str) {
		self.calls.lock().unwrap().push(name);
	}
}
#[async_trait]
impl ToolOperations for Operations {
	async fn connection_call(
		&self,
		call: &aidash_domain::identity::commands::connections::ConnectionCall,
	) -> Result<Option<Value>> {
		if let Some(result) = &self.proxy {
			self.record("home.call");
			self.inputs
				.lock()
				.unwrap()
				.push((call.idempotency_key.clone(), serde_json::to_value(call)?));
			Ok(Some(result.clone()))
		} else {
			Ok(None)
		}
	}

	async fn connection_person(&self) -> Result<bool> {
		Ok(self.person)
	}

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
	async fn human_request(&self, kind: &str, prompt: &str, key: &str) -> Result<HumanRequest> {
		self.record("human.request");
		self.inputs.lock().unwrap().push((
			key.into(),
			json!({"kind":kind,"prompt":serde_json::from_str::<Value>(prompt)?}),
		));
		Ok(HumanRequest {
			id: Uuid::now_v7(),
			run_id: Uuid::nil(),
			workspace_id: Uuid::nil(),
			kind: kind.into(),
			prompt: prompt.into(),
			response: None,
			created_at: chrono::Utc::now(),
			answered_by: None,
			answer_by: None,
		})
	}
	async fn binding_specifications(&self) -> Result<BTreeMap<String, ToolSpec>> {
		panic!("unexpected tool effect: binding_specifications")
	}
	async fn direct_skills(&self) -> Result<Vec<aidash_domain::exposure::DirectSkill>> {
		panic!("unexpected tool effect: direct_skills")
	}
	async fn direct_skill_file(&self, _: Uuid, _: &str, _: &str) -> Result<Option<Vec<u8>>> {
		panic!("unexpected tool effect: direct_skill_file")
	}
	fn skill_read_bytes(&self) -> Result<usize> {
		panic!("unexpected tool effect: skill_read_bytes")
	}
	async fn connection(
		&self,
		resource: &str,
		_call: SecretCall,
		_key: &str,
	) -> Result<CallSecret> {
		self.record("connection.resolve");
		assert_eq!(resource, "https://api.example.com/mcp");
		if !self.connection {
			return Err(Error::Invalid(
				aidash_domain::oauth_connections::CONNECTION_REQUIRED.into(),
			));
		}
		Ok(CallSecret::Connection {
			record: crate::tenant_secrets::ConnectionRecord {
				id: Uuid::now_v7(),
				resource: resource.into(),
				host: "api.example.com".into(),
			},
			source: std::sync::Arc::new(Token),
		})
	}

	async fn tenant_secret(
		&self,
		name: &str,
		endpoint: &str,
		call: SecretCall,
		key: &str,
	) -> Result<CallSecret> {
		self.record("secret.resolve");
		self.inputs.lock().unwrap().push((
			format!("{name}@{endpoint}"),
			json!({
				"key": key,
				"methods": call.methods,
				"subject": call.subject,
			}),
		));
		if name == "missing" {
			return Err(Error::Invalid(
				aidash_domain::tenant_secrets::UNAVAILABLE.into(),
			));
		}
		Ok(CallSecret::Value {
			record: SecretRecord {
				id: Uuid::now_v7(),
				version: "1".into(),
				host: "api.example.test".into(),
			},
			value: secrecy::SecretString::from("tenant-secret-value".to_owned()),
		})
	}
}
#[async_trait]
impl ToolTransport for Operations {
	async fn invoke(
		&self,
		_config: &ToolConfig,
		input: Value,
		key: &str,
		secret: Option<&CallSecret>,
	) -> Result<Value> {
		self.record(if secret.is_some() {
			"transport.invoke.with_secret"
		} else {
			"transport.invoke"
		});
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
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
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
		control_revision: 0,
		control_source: Default::default(),
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
			proxy: None,
			skill_text: "界ab".into(),
			connection: false,
			person: false,
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
#[case("http://allowed.invalid/path")]
#[case("https://allowed.invalid:8443/path")]
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

#[test]
fn native_http_rejects_literal_ip_hosts_even_when_explicitly_allowlisted() {
	for (url, host) in [
		("https://127.0.0.1/", "127.0.0.1"),
		("https://[::1]/", "[::1]"),
	] {
		assert!(validate_native_http_url(url, &[host.to_owned()]).is_err());
	}
}
#[rstest]
#[tokio::test]
async fn schema_rejection_precedes_every_tool_effect(fixture: Fixture) {
	// Arrange
	let tool = plugin(
		ToolConfig::Http {
			endpoint: "http://fixture.invalid".into(),
			replay: "idempotent".into(),
			secret: None,
			secret_owner: Default::default(),
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

fn secret_tool(secret: Option<&str>) -> Plugin {
	plugin(
		ToolConfig::Http {
			endpoint: "https://api.example.com/run".into(),
			replay: "idempotent".into(),
			secret: secret.map(str::to_owned),
			secret_owner: Default::default(),
		},
		json!({"type":"object"}),
	)
}
#[rstest]
#[tokio::test]
async fn a_tool_that_names_a_tenant_secret_sends_its_value_to_its_own_endpoint(fixture: Fixture) {
	// Arrange
	let tool = secret_tool(Some("search"));
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
	// Assert: the value is resolved for exactly that endpoint, then handed to the transport.
	assert!(result.is_ok());
	assert_eq!(
		*fixture.operations.calls.lock().unwrap(),
		["secret.resolve", "transport.invoke.with_secret"]
	);
	assert_eq!(
		fixture.operations.inputs.lock().unwrap()[0].0,
		"search@https://api.example.com/run"
	);
	assert_eq!(
		fixture.operations.inputs.lock().unwrap()[0].1["methods"],
		json!(["POST"])
	);
	assert_eq!(
		fixture.operations.inputs.lock().unwrap()[0].1["subject"]["run"],
		fixture.run.id.to_string()
	);
	assert_eq!(fixture.operations.inputs.lock().unwrap()[0].1["key"], "key");
}
#[rstest]
#[tokio::test]
async fn a_tool_without_a_tenant_secret_resolves_none(fixture: Fixture) {
	let result = secret_tool(None)
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
	assert!(result.is_ok());
	assert_eq!(
		*fixture.operations.calls.lock().unwrap(),
		["transport.invoke"]
	);
}
#[rstest]
#[tokio::test]
async fn an_unavailable_tenant_secret_fails_the_call_before_any_request(fixture: Fixture) {
	let result = secret_tool(Some("missing"))
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
	assert!(
		matches!(&result, Err(Error::Invalid(message)) if message == "secret_unavailable"),
		"{result:?}"
	);
	assert_eq!(
		*fixture.operations.calls.lock().unwrap(),
		["secret.resolve"]
	);
}

struct Token;
#[async_trait]
impl crate::tenant_secrets::BearerSource for Token {
	async fn bearer(&self) -> Result<secrecy::SecretString> {
		Ok("connection-token".into())
	}
	async fn invalidate(&self) {}
}

/// The orchestration boundary owns precedence; wire tests own challenge/refresh behavior.
#[rstest]
#[case::connection_beats_named_secret(true, Some("search"), vec!["connection.resolve", "transport.invoke.with_secret"])]
#[case::missing_connection_uses_secret(false, Some("search"), vec!["connection.resolve", "secret.resolve", "transport.invoke.with_secret"])]
#[case::no_credentials_is_anonymous(false, None, vec!["connection.resolve", "transport.invoke"])]
#[tokio::test]
async fn mcp_authentication_resolution_order(
	mut fixture: Fixture,
	#[case] connected: bool,
	#[case] secret: Option<&str>,
	#[case] expected: Vec<&str>,
) {
	fixture.operations.connection = connected;
	let mut tool = secret_tool(None);
	tool.config = ToolConfig::Mcp {
		endpoint: "https://api.example.com/mcp/".into(),
		tool_name: "lookup".into(),
		replay: "read_only".into(),
		secret: secret.map(str::to_owned),
		idempotency_argument: None,
		scopes: None,
		secret_owner: Default::default(),
	};
	assert!(
		tool.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run
			},
			&fixture.operations,
			json!({}),
			"mcp-key"
		)
		.await
		.is_ok()
	);
	assert_eq!(*fixture.operations.calls.lock().unwrap(), expected);
}

#[rstest]
#[case::http(false, false)]
#[case::mcp(false, true)]
#[case::connection_precedes_personal_secret(true, true)]
#[tokio::test]
async fn personal_only_tools_never_use_an_ownerless_shared_secret(
	mut fixture: Fixture,
	#[case] connected: bool,
	#[case] mcp: bool,
) {
	fixture.operations.connection = connected;
	let mut tool = secret_tool(Some("search"));
	tool.config = if mcp {
		ToolConfig::Mcp {
			endpoint: "https://api.example.com/mcp".into(),
			tool_name: "lookup".into(),
			replay: "idempotent".into(),
			idempotency_argument: None,
			secret: Some("search".into()),
			scopes: None,
			secret_owner: aidash_domain::tool::SecretOwner::Personal,
		}
	} else {
		ToolConfig::Http {
			endpoint: "https://api.example.com/tool".into(),
			replay: "idempotent".into(),
			secret: Some("search".into()),
			secret_owner: aidash_domain::tool::SecretOwner::Personal,
		}
	};
	let result = tool
		.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run,
			},
			&fixture.operations,
			json!({}),
			"personal-call",
		)
		.await
		.unwrap();
	if connected {
		assert!(
			fixture
				.operations
				.calls
				.lock()
				.unwrap()
				.contains(&"transport.invoke.with_secret")
		);
	} else {
		assert_eq!(result["error"], "secret_unavailable");
		assert!(result["hint"].as_str().unwrap().contains("personal secret"));
		assert!(
			!fixture
				.operations
				.calls
				.lock()
				.unwrap()
				.iter()
				.any(|call| call.starts_with("transport.invoke"))
		);
	}
}

#[rstest]
#[case::http_any(false, aidash_domain::tool::SecretOwner::Any, false)]
#[case::http_personal(false, aidash_domain::tool::SecretOwner::Personal, false)]
#[case::mcp(true, aidash_domain::tool::SecretOwner::Any, false)]
#[case::literal_fallback(false, aidash_domain::tool::SecretOwner::Any, true)]
#[tokio::test]
async fn a_home_personal_call_returns_its_result_before_any_local_secret_or_transport(
	mut fixture: Fixture,
	#[case] mcp: bool,
	#[case] owner: aidash_domain::tool::SecretOwner,
	#[case] literal_fallback: bool,
) {
	let result = if literal_fallback {
		json!({"outcome":"not_personal"})
	} else {
		json!({"content":[{"type":"text","text":"home result"}]})
	};
	fixture.operations.proxy = Some(result.clone());
	let config = if mcp {
		ToolConfig::Mcp {
			endpoint: "https://api.example.com/mcp".into(),
			tool_name: "lookup".into(),
			replay: "idempotent".into(),
			idempotency_argument: None,
			secret: None,
			scopes: None,
			secret_owner: owner,
		}
	} else {
		ToolConfig::Http {
			endpoint: "https://api.example.com/run".into(),
			replay: "idempotent".into(),
			secret: Some("personal".into()),
			secret_owner: owner,
		}
	};
	let tool = plugin(config, json!({"type":"object"}));
	assert_eq!(
		tool.invoke(
			&ToolContext {
				operations: &fixture.operations,
				run: &fixture.run
			},
			&fixture.operations,
			json!({"query":"private"}),
			"logical-key"
		)
		.await
		.unwrap(),
		result
	);
	assert_eq!(*fixture.operations.calls.lock().unwrap(), ["home.call"]);
	let saved = fixture.operations.inputs.lock().unwrap();
	assert_eq!(saved[0].1["arguments"], json!({"query":"private"}));
	assert_eq!(saved[0].1["idempotency_key"], "logical-key");
	assert_eq!(
		saved[0].1["kind"],
		if mcp { "connection" } else { "personal_secret" }
	);
	assert!(saved[0].1.get("authorization").is_none());
	assert!(saved[0].1.get("token").is_none());
}

#[rstest]
#[case::fallback(json!({"outcome":"not_personal"}))]
#[case::connection(json!({"error":"connection_required","resource":"https://mcp.example/tools"}))]
#[case::scope(json!({"error":"connection_scope_insufficient","resource":"https://mcp.example/tools"}))]
#[case::null(Value::Null)]
fn connection_envelopes_preserve_every_literal_tool_result(#[case] result: Value) {
	assert_eq!(
		connection_outcome(json!({"outcome":"result","result":result}), false).unwrap(),
		Some(result)
	);
}
#[test]
fn typed_connection_envelopes_restore_the_error_provenance() {
	let cause = aidash_domain::agent_plugin::ConnectionCause::ConnectionRequired {
		resource: "https://mcp.example/tools".into(),
	};
	assert!(
		matches!(connection_outcome(json!({"outcome":"connection","cause":cause}), false), Err(Error::Connection(received)) if received == cause)
	);
	assert_eq!(
		connection_outcome(json!({"outcome":"not_personal"}), false).unwrap(),
		None
	);
}

#[rstest]
#[tokio::test]
async fn connection_request_canonicalizes_consent_and_refuses_without_a_person(
	mut fixture: Fixture,
) {
	let builtin = builtins().remove("connection_request").unwrap();
	let input = json!({"resource":"https://MCP.example:443/mcp/","scopes":["write","read","read"],"reason":"This task needs documents"});
	let ctx = ToolContext {
		operations: &fixture.operations,
		run: &fixture.run,
	};
	assert!(
		builtin
			.invoke(&ctx, input.clone(), "connect")
			.await
			.is_err()
	);
	assert!(fixture.operations.calls.lock().unwrap().is_empty());
	fixture.operations.person = true;
	let ctx = ToolContext {
		operations: &fixture.operations,
		run: &fixture.run,
	};
	let result = builtin.invoke(&ctx, input, "connect").await.unwrap();
	assert!(result["human_request_id"].is_string());
	let inputs = fixture.operations.inputs.lock().unwrap();
	let (_, request) = &inputs[0];
	assert_eq!(
		request["kind"],
		aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND
	);
	assert_eq!(request["prompt"]["resource"], "https://mcp.example/mcp");
	assert_eq!(request["prompt"]["scopes"], json!(["read", "write"]));
}

#[rstest]
#[case("http://ollama.lan:11434/mcp", None)]
#[case("https://api.example.com/mcp?tenant=one", Some("search"))]
#[tokio::test]
async fn non_oauth_mcp_endpoint_uses_existing_transport(
	fixture: Fixture,
	#[case] endpoint: &str,
	#[case] secret: Option<&str>,
) {
	let mut tool = secret_tool(None);
	tool.config = ToolConfig::Mcp {
		endpoint: endpoint.into(),
		tool_name: "lookup".into(),
		replay: "read_only".into(),
		secret: secret.map(str::to_owned),
		idempotency_argument: None,
		scopes: None,
		secret_owner: Default::default(),
	};
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
	assert!(result.is_ok(), "{result:?}");
	let calls = fixture.operations.calls.lock().unwrap();
	assert!(!calls.contains(&"connection.resolve"));
	assert!(calls.contains(&if secret.is_some() {
		"transport.invoke.with_secret"
	} else {
		"transport.invoke"
	}));
}
