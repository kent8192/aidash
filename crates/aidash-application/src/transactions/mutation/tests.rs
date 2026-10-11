use super::*;
use crate::ports::{
	Credentials,
	registry::{CoreToolCatalog, DefinitionLookup, DefinitionWriter},
};
use aidash_domain::{
	Artifact, ArtifactInput, RecoveryState, RunControl, RunPhase, RunState, Task, Workspace,
	capabilities::CoreCapabilities,
	context::{Context, ContextUsage},
	provider::{ToolCall, ToolSpec},
	registry::Entry,
	run_state::{RawRun, ToolCallState, encode},
	transactions::{Isolation, Participant},
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

struct Catalog;
impl Credentials for Catalog {
	fn resolve(&self, name: &str) -> Result<String> {
		Err(Error::NotFound(name.into()))
	}
}
impl CoreToolCatalog for Catalog {
	fn specifications(&self, _config: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(Catalog), Arc::new(Catalog))
}

#[fixture]
fn task() -> Task {
	serde_json::from_value(json!({"id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"title":"Task","description":"Complete atomically","status":"RUNNING","requirements":{},"owner":"aidash://worker/agents/agent@1","created_by":"operator","dependencies":[],"parent_id":null,"revision":3,"created_at":"2030-01-01T00:00:00Z"})).unwrap()
}

#[fixture]
fn run() -> RawRun {
	let context = Context {
		usage: Some(ContextUsage {
			input_tokens: 11,
			output_tokens: 4,
			context_window: 128000,
			compactions: 2,
			exposure: None,
		}),
		..Default::default()
	};
	RawRun {
		metadata: serde_json::from_value(json!({"id":Uuid::from_u128(4),"task_id":Uuid::from_u128(1),"workspace_id":Uuid::from_u128(2),"home_node":"aidash://home","agent_id":"agent","agent_version":"1","phase":"TOOL_CALL","control":"ACTIVE","step":7,"revision":5,"observed_input_seq":0,"ledger_worker_ready":false,"error":null,"lease_owner":null,"lease_until":null,"updated_at":"2030-01-01T00:00:00Z"})).unwrap(),
		context: json!(context),
		pending: encode(&RunState::ToolCall(Box::default()), &RecoveryState::default()).unwrap(),
	}
}

fn artifact_input() -> ArtifactInput {
	ArtifactInput {
		kind: "text".into(),
		name: "Answer".into(),
		content: json!("Done"),
	}
}
fn completion() -> Mutation {
	Mutation::CompleteTask {
		task_id: Uuid::from_u128(1),
		expected_revision: 3,
		artifact: artifact_input(),
	}
}
fn finalization() -> Mutation {
	Mutation::FinishRun {
		run_id: Uuid::from_u128(4),
		task_id: Uuid::from_u128(1),
		expected_revision: 5,
	}
}
fn manifest(local: &str, mutations: Vec<Mutation>, other: Option<Participant>) -> Manifest {
	let mut participants = vec![Participant {
		node_id: local.into(),
		mutations,
	}];
	participants.extend(other);
	Manifest {
		id: Uuid::from_u128(9),
		coordinator: "aidash://home".into(),
		isolation: Isolation::Serializable,
		deadline: "2030-01-01T00:00:00Z".parse().unwrap(),
		participants,
	}
}

struct Scope {
	task: Task,
	run: RawRun,
	children: bool,
	delegated: Option<String>,
	source: Option<Uuid>,
	leased: bool,
	inserted: bool,
	failure: Option<&'static str>,
	calls: Vec<&'static str>,
	keys: Vec<String>,
	outputs: Vec<(Uuid, Uuid, String, Uuid)>,
	events: Vec<(String, Option<Uuid>, String, Value)>,
}
#[fixture]
fn scope(task: Task, run: RawRun) -> Scope {
	Scope {
		task,
		run,
		children: false,
		delegated: None,
		source: None,
		leased: false,
		inserted: true,
		failure: None,
		calls: vec![],
		keys: vec![],
		outputs: vec![],
		events: vec![],
	}
}
impl Scope {
	fn call(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name);
		if self.failure == Some(name) {
			return Err(Error::External(format!("fault:{name}")));
		}
		Ok(())
	}
}
#[async_trait]
impl DefinitionLookup for Scope {
	async fn definition(&mut self, id: &str, _version: &str) -> Result<Entry> {
		Err(Error::NotFound(id.into()))
	}
	async fn overrides(&mut self, _id: &str, _version: &str) -> Result<Option<Value>> {
		Ok(None)
	}
}
#[async_trait]
impl DefinitionWriter for Scope {
	async fn insert_definition(&mut self, _entry: &Entry) -> Result<bool> {
		self.call("definition")?;
		Ok(self.inserted)
	}
}
#[async_trait]
impl MutationScope for Scope {
	async fn replace_workspace(
		&mut self,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<Workspace> {
		self.call("workspace")?;
		Ok(Workspace {
			id,
			title: "Workspace".into(),
			goal: "Goal".into(),
			state,
			revision: revision + 1,
			created_at: self.task.created_at,
		})
	}
	async fn lock_task(&mut self, id: Uuid) -> Result<Task> {
		self.call("task")?;
		assert_eq!(id, self.task.id);
		Ok(self.task.clone())
	}
	async fn unfinished_children(&mut self, id: Uuid) -> Result<bool> {
		self.call("children")?;
		assert_eq!(id, self.task.id);
		Ok(self.children)
	}
	async fn delegated_node(&mut self, id: Uuid) -> Result<Option<String>> {
		self.call("delegation")?;
		assert_eq!(id, self.task.id);
		Ok(self.delegated.clone())
	}
	async fn complete_task(
		&mut self,
		task: &Task,
		artifact: &ArtifactInput,
		key: &str,
	) -> Result<(Task, Artifact)> {
		self.call("complete_task")?;
		self.keys.push(key.into());
		let mut saved = task.clone();
		saved.status = aidash_domain::TaskStatus::Completed;
		saved.revision += 1;
		let created = Artifact {
			id: Uuid::from_u128(8),
			workspace_id: task.workspace_id,
			task_id: task.id,
			kind: artifact.kind.clone(),
			name: artifact.name.clone(),
			content: artifact.content.clone(),
			created_by: task.owner.clone().unwrap(),
			idempotency_key: key.into(),
			created_at: task.created_at,
		};
		Ok((saved, created))
	}
	async fn source_run(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		self.call("source")?;
		assert_eq!(task, self.task.id);
		Ok(self.source)
	}
	async fn record_output(
		&mut self,
		run: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		self.call("output")?;
		self.outputs.push((run, workspace, kind.into(), id));
		Ok(())
	}
	async fn lock_run(&mut self, id: Uuid) -> Result<(RawRun, bool)> {
		self.call("run")?;
		assert_eq!(id, self.run.id);
		Ok((self.run.clone(), self.leased))
	}
	async fn complete_run(&mut self, id: Uuid) -> Result<()> {
		self.call("complete_run")?;
		assert_eq!(id, self.run.id);
		Ok(())
	}
	async fn append_event(
		&mut self,
		node: &str,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<()> {
		self.call("event")?;
		self.events
			.push((node.into(), workspace, kind.into(), data));
		Ok(())
	}
}

#[rstest]
#[case::stale_revision("revision", "task must be running at its expected revision", & ["task"])]
#[case::unowned("owner", "task must be running at its expected revision", & ["task"])]
#[case::children("children", "task still has unfinished children", & ["task", "children"])]
#[tokio::test]
async fn task_guards_precede_delegation_and_writes(
	mut scope: Scope,
	#[case] change: &str,
	#[case] message: &str,
	#[case] calls: &[&str],
) {
	match change {
		"revision" => scope.task.revision += 1,
		"owner" => scope.task.owner = None,
		_ => scope.children = true,
	}
	let manifest = manifest("aidash://home", vec![completion()], None);
	let error = apply(&mut scope, &validation(), "aidash://home", &manifest)
		.await
		.unwrap_err();
	assert_eq!(error.to_string(), message);
	assert_eq!(scope.calls, calls);
	assert!(scope.events.is_empty());
}

#[rstest]
#[case::missing_participant(None, "forbidden")]
#[case::missing_pair(Some(vec![]), "delegated completion requires its participant's execution finalization")]
#[case::another_task(Some(vec![Mutation::FinishRun { run_id: Uuid::from_u128(4), task_id: Uuid::from_u128(7), expected_revision: 5 }]), "delegated completion requires its participant's execution finalization")]
#[tokio::test]
async fn delegated_task_requires_its_executor_pair(
	mut scope: Scope,
	#[case] mutations: Option<Vec<Mutation>>,
	#[case] message: &str,
) {
	scope.delegated = Some("aidash://worker".into());
	let peer = mutations.map(|mutations| Participant {
		node_id: "aidash://worker".into(),
		mutations,
	});
	let manifest = manifest("aidash://home", vec![completion()], peer);
	assert_eq!(
		apply(&mut scope, &validation(), "aidash://home", &manifest)
			.await
			.unwrap_err()
			.to_string(),
		message
	);
	assert_eq!(scope.calls, ["task", "children", "delegation"]);
	assert!(scope.keys.is_empty());
}

#[rstest]
#[tokio::test]
async fn task_completion_keeps_provenance_and_event_after_the_selective_write(mut scope: Scope) {
	scope.delegated = Some("aidash://worker".into());
	scope.source = Some(Uuid::from_u128(6));
	let manifest = manifest(
		"aidash://home",
		vec![completion()],
		Some(Participant {
			node_id: "aidash://worker".into(),
			mutations: vec![finalization()],
		}),
	);
	apply(&mut scope, &validation(), "aidash://home", &manifest)
		.await
		.unwrap();
	let key = format!("atomic:{}:task:{}", manifest.id, scope.task.id);
	assert_eq!(scope.keys.as_slice(), std::slice::from_ref(&key));
	assert_eq!(
		scope.outputs,
		[(
			Uuid::from_u128(6),
			scope.task.workspace_id,
			"artifact".into(),
			Uuid::from_u128(8)
		)]
	);
	assert_eq!(
		scope.calls,
		[
			"task",
			"children",
			"delegation",
			"complete_task",
			"source",
			"output",
			"event"
		]
	);
	let mut task = json!(scope.task);
	task["revision"] = json!(4);
	task["status"] = json!("COMPLETED");
	assert_eq!(
		scope.events,
		[(
			"aidash://home".into(),
			Some(scope.task.workspace_id),
			"task.completed".into(),
			json!({"task":task,"artifact":{"id":Uuid::from_u128(8),"workspace_id":scope.task.workspace_id,"task_id":scope.task.id,"kind":"text","name":"Answer","content":"Done","created_by":scope.task.owner,"idempotency_key":key,"created_at":scope.task.created_at}})
		)]
	);
}

#[rstest]
#[case::revision("revision")]
#[case::phase("phase")]
#[case::cancelled("cancelled")]
#[case::lease("lease")]
#[case::task("task")]
#[tokio::test]
async fn run_preconditions_take_precedence_over_corrupt_execution_state(
	mut scope: Scope,
	#[case] change: &str,
) {
	scope.run.pending = json!({"broken":true});
	scope.run.context = Value::Null;
	match change {
		"revision" => scope.run.metadata.revision += 1,
		"phase" => scope.run.metadata.phase = RunPhase::Thinking,
		"cancelled" => scope.run.metadata.control = RunControl::Cancelled,
		"lease" => scope.leased = true,
		_ => scope.run.metadata.task_id = Uuid::from_u128(7),
	}
	let manifest = manifest("aidash://worker", vec![finalization()], None);
	assert!(
		matches!(apply(&mut scope, &validation(), "aidash://worker", &manifest).await, Err(Error::Domain(aidash_domain::Error::Conflict(message))) if message == "run must be quiescent at its expected tool-call revision")
	);
	assert_eq!(scope.calls, ["run"]);
}

#[rstest]
#[case::corrupt("corrupt", "invalid pending envelope or state version")]
#[case::pending_tools("tools", "run still has pending tools")]
#[tokio::test]
async fn execution_state_is_checked_before_home_pair_and_writes(
	mut scope: Scope,
	#[case] change: &str,
	#[case] message: &str,
) {
	if change == "corrupt" {
		scope.run.pending = Value::Null;
	} else {
		let state = ToolCallState {
			response: aidash_domain::provider::ModelResponse {
				tool_calls: vec![ToolCall {
					id: "call".into(),
					name: "echo".into(),
					arguments: json!({}),
				}],
				..Default::default()
			},
			..Default::default()
		};
		scope.run.pending = encode(
			&RunState::ToolCall(Box::new(state)),
			&RecoveryState::default(),
		)
		.unwrap();
	}
	let manifest = manifest("aidash://worker", vec![finalization()], None);
	assert_eq!(
		apply(&mut scope, &validation(), "aidash://worker", &manifest)
			.await
			.unwrap_err()
			.to_string(),
		message
	);
	assert_eq!(scope.calls, ["run"]);
}

#[rstest]
#[case::missing_home(None, "forbidden")]
#[case::missing_task(Some(vec![]), "execution finalization requires the home task's atomic completion")]
#[tokio::test]
async fn run_completion_requires_the_home_task_pair(
	mut scope: Scope,
	#[case] mutations: Option<Vec<Mutation>>,
	#[case] message: &str,
) {
	let home = mutations.map(|mutations| Participant {
		node_id: "aidash://home".into(),
		mutations,
	});
	let manifest = manifest("aidash://worker", vec![finalization()], home);
	assert_eq!(
		apply(&mut scope, &validation(), "aidash://worker", &manifest)
			.await
			.unwrap_err()
			.to_string(),
		message
	);
	assert_eq!(scope.calls, ["run"]);
}

#[rstest]
#[case::local("aidash://home", Some(Uuid::from_u128(2)))]
#[case::foreign("aidash://worker", None)]
#[tokio::test]
async fn execution_event_preserves_raw_usage_and_home_visibility(
	mut scope: Scope,
	#[case] node: &str,
	#[case] workspace: Option<Uuid>,
) {
	let (mutations, other) = if node == "aidash://home" {
		(vec![finalization(), completion()], None)
	} else {
		(
			vec![finalization()],
			Some(Participant {
				node_id: "aidash://home".into(),
				mutations: vec![completion()],
			}),
		)
	};
	let manifest = manifest(node, mutations, other);
	apply(&mut scope, &validation(), node, &manifest)
		.await
		.unwrap();
	assert_eq!(
		scope.events[0],
		(
			node.into(),
			workspace,
			"run.completed".into(),
			json!({"run_id":scope.run.id,"task_id":scope.run.task_id,"workspace_id":scope.run.workspace_id,"agent_id":"agent","phase":"COMPLETED","step":7,"error":null,"context_usage":{"input_tokens":11,"output_tokens":4,"context_window":128000,"compactions":2}})
		)
	);
	assert_eq!(&scope.calls[..3], &["run", "complete_run", "event"]);
}

#[rstest]
#[case::source("source", & ["task", "children", "delegation", "complete_task", "source"])]
#[case::provenance("output", & ["task", "children", "delegation", "complete_task", "source", "output"])]
#[tokio::test]
async fn provenance_failures_stop_event_publication(
	mut scope: Scope,
	#[case] failure: &'static str,
	#[case] calls: &[&str],
) {
	scope.source = Some(Uuid::from_u128(6));
	scope.failure = Some(failure);
	let manifest = manifest("aidash://home", vec![completion()], None);
	assert_eq!(
		apply(&mut scope, &validation(), "aidash://home", &manifest)
			.await
			.unwrap_err()
			.to_string(),
		format!("fault:{failure}")
	);
	assert_eq!(scope.calls, calls);
	assert!(scope.events.is_empty());
}

#[rstest]
#[case::insert(true)]
#[case::replay(false)]
#[tokio::test]
async fn registry_replay_emits_no_duplicate_event_and_workspace_keeps_its_json(
	mut scope: Scope,
	#[case] inserted: bool,
) {
	scope.inserted = inserted;
	let entry: Entry = serde_json::from_value(json!({"id":"skill","version":"1.0.0","kind":"skill","name":{"en":"Skill"},"description":{"en":"Atomic registration"},"config":{"instructions":"Follow the task"}})).unwrap();
	let manifest = manifest(
		"aidash://home",
		vec![
			Mutation::RegistryRegister {
				entry: Box::new(entry),
			},
			Mutation::WorkspaceState {
				workspace_id: scope.task.workspace_id,
				expected_revision: 6,
				state: json!({"ready":true,"value":null}),
			},
		],
		None,
	);
	apply(&mut scope, &validation(), "aidash://home", &manifest)
		.await
		.unwrap();
	let mut expected = vec![];
	if inserted {
		expected.push((
			"aidash://home".into(),
			None,
			"registry.registered".into(),
			json!({"id":"skill","version":"1.0.0"}),
		));
	}
	expected.push(("aidash://home".into(), Some(scope.task.workspace_id), "workspace.updated".into(), json!({"id":scope.task.workspace_id,"title":"Workspace","goal":"Goal","state":{"ready":true,"value":null},"revision":7,"created_at":scope.task.created_at})));
	assert_eq!(scope.events, expected);
}
