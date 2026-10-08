use super::*;
use crate::ports::authorization::commands::RemoteCommandScope;
use aidash_domain::{
	Message, Task, identity::commands::Binding, policy::Resource, qualified_agent,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::collections::HashMap;
struct Scope {
	task: Task,
	additional: HashMap<Uuid, Task>,
	previous: Option<(String, Value)>,
	calls: Vec<String>,
	applied: Vec<Value>,
	journal: Vec<Value>,
	history: Vec<Message>,
	live: bool,
	created: bool,
	failure: Option<String>,
	output: Option<Uuid>,
	record: Value,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		task: Task {
			id: Uuid::from_u128(2),
			workspace_id: Uuid::from_u128(3),
			title: String::new(),
			description: String::new(),
			status: TaskStatus::Claimed,
			requirements: json!({}),
			owner: Some(qualified_agent("aidash://peer", "agent", "1")),
			created_by: "root".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 8,
			created_at: chrono::Utc::now(),
		},
		additional: HashMap::new(),
		previous: None,
		calls: vec![],
		applied: vec![],
		journal: vec![],
		history: vec![],
		live: true,
		created: true,
		failure: None,
		output: Some(Uuid::from_u128(9)),
		record: json!({"visible":true}),
	}
}
impl Scope {
	fn call(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.failure.as_deref() == Some(name) {
			Err(Error::External(format!("fault:{name}")))
		} else {
			Ok(())
		}
	}
	fn res(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "source".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	fn child(&self) -> Task {
		Task {
			id: Uuid::from_u128(5),
			parent_id: Some(self.task.id),
			created_by: self.task.owner.clone().unwrap(),
			owner: None,
			status: TaskStatus::Open,
			..self.task.clone()
		}
	}
}
#[async_trait]
impl RemoteCommandScope for Scope {
	async fn binding(&mut self, _: Uuid) -> Result<Option<Binding>> {
		self.call("binding")?;
		Ok(Some(Binding {
			admission_id: Uuid::from_u128(1),
			task_id: self.task.id,
		}))
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		self.call(&format!("task:{id}"))?;
		Ok(if id == self.task.id {
			self.task.clone()
		} else {
			self.additional.get(&id).expect("fixture task").clone()
		})
	}
	async fn previous(&mut self, _: Uuid, key: &str) -> Result<Option<(String, Value)>> {
		self.call(&format!("previous:{key}"))?;
		Ok(self.previous.clone())
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.call("task_resource")?;
		Ok(self.res("task", &task.id.to_string(), json!({"saved":"attributes"})))
	}
	async fn require_builtin(&mut self, tool: &str) -> Result<()> {
		self.call(&format!("builtin:{tool}"))
	}
}
#[async_trait]
impl RemoteCommandEffects for Scope {
	async fn human_read(
		&mut self,
		_: Uuid,
		_: Uuid,
		_: Uuid,
	) -> Result<aidash_domain::HumanRequest> {
		Err(Error::Forbidden)
	}
	fn local_node(&self) -> &str {
		"aidash://home"
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.res(kind, id, attributes)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		if action == "run.message" {
			assert_eq!(resource.kind, "run");
			assert_eq!(resource.id, Uuid::from_u128(1).to_string());
			assert_eq!(resource.attributes, json!({"saved":"attributes"}));
		}
		self.call(&format!("require:{action}"))
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		assert_eq!(id, self.task.workspace_id);
		self.call("workspace")?;
		Ok(self.res("workspace", &id.to_string(), json!({})))
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, owner: &str) -> Result<Resource> {
		assert_eq!(task, self.task.id);
		assert_eq!(owner, self.task.owner.as_deref().unwrap());
		self.call("artifact_resource")?;
		Ok(self.res("artifact", "new", json!({})))
	}
	async fn snapshot(&mut self, _: Uuid) -> Result<Value> {
		self.call("snapshot")?;
		Ok(json!({"snapshot":"visible"}))
	}
	async fn record(&mut self, _: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		self.call(&format!("record:{kind}:{id}"))?;
		Ok(self.record.clone())
	}
	async fn children(&mut self, _: Uuid, parent: Uuid) -> Result<Value> {
		assert_eq!(parent, self.task.id);
		self.call("children")?;
		Ok(json!([]))
	}
	async fn created_by_grant(&mut self, _: Uuid, child: Uuid) -> Result<bool> {
		assert_eq!(child, Uuid::from_u128(5));
		self.call("provenance")?;
		Ok(self.created)
	}
	async fn apply(&mut self, context: WriteContext<'_>, effect: Effect<'_>) -> Result<Applied> {
		self.call("apply")?;
		let effect = match effect {
			Effect::HumanRequest { kind, prompt } => {
				json!({"kind":"human_request","request_kind":kind,"prompt":prompt})
			}
			Effect::Claim { revision, agent } => {
				json!({"kind":"claim","revision":revision,"agent":agent})
			}
			Effect::Transition {
				revision,
				next,
				through,
			} => json!({"kind":"transition","revision":revision,"next":next,"through":through}),
			Effect::Artifact {
				artifact,
				complete_through,
			} => json!({"kind":"artifact","artifact":artifact,"through":complete_through}),
			Effect::CreateTask { task } => json!({"kind":"create_task","task":task}),
			Effect::Delegate { child, agent } => {
				json!({"kind":"delegate","child":child,"agent":agent})
			}
			Effect::Message { content, through } => {
				json!({"kind":"message","content":content,"through":through})
			}
			Effect::ReserveInput { node, key, content } => {
				json!({"kind":"reserve","node":node,"key":key,"content":content})
			}
			Effect::CommitInput { key, content, seq } => {
				json!({"kind":"commit","key":key,"content":content,"seq":seq})
			}
			Effect::DeliverInput { node, key, content } => {
				json!({"kind":"deliver","node":node,"key":key,"content":content})
			}
			Effect::ReleaseInputs { node, keys } => {
				json!({"kind":"release","node":node,"keys":keys})
			}
			Effect::Event { kind, data } => json!({"kind":"event","event_kind":kind,"data":data}),
		};
		self.applied.push(json!({"effect":effect,"task":context.task.id,"owner":context.owner,"key":context.key,"admission":context.admission}));
		Ok(Applied {
			value: json!({"saved":true}),
			output_id: self.output,
		})
	}
	async fn record_output(
		&mut self,
		grant: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		assert_eq!(grant, Uuid::from_u128(4));
		assert_eq!(workspace, self.task.workspace_id);
		assert_eq!(id, Uuid::from_u128(9));
		self.call(&format!("output:{kind}"))
	}
	async fn history(
		&mut self,
		_: Uuid,
		task: Uuid,
		node: &str,
		offset: usize,
	) -> Result<Vec<Message>> {
		assert_eq!(task, self.task.id);
		assert_eq!(node, "aidash://peer");
		assert_eq!(offset, 7);
		self.call("history")?;
		Ok(self.history.clone())
	}
	async fn persist_replay(
		&mut self,
		grant: Uuid,
		task: Uuid,
		key: &str,
		digest: &str,
		result: &Value,
	) -> Result<()> {
		self.call("journal")?;
		self.journal
			.push(json!({"grant":grant,"task":task,"key":key,"digest":digest,"result":result}));
		Ok(())
	}
	async fn live(&mut self, _: Uuid) -> Result<bool> {
		self.call("live")?;
		Ok(self.live)
	}
}
fn input<'a>(operation: &'a str, data: &'a Value) -> Command<'a> {
	Command {
		grant_id: Uuid::from_u128(4),
		admission_id: Uuid::from_u128(1),
		operation,
		data,
		node: "aidash://peer",
		agent: "agent",
		version: "1",
	}
}
fn agent() -> Entry {
	serde_json::from_value(json!({"id":"agent","version":"1","kind":"agent","name":{"en":"Fixture agent"},"description":{"en":"Fixture description"},"config":{"tools":["saved-tool"]}})).unwrap()
}
async fn run(scope: &mut Scope, operation: &str, data: &Value) -> Result<Value> {
	execute(scope, input(operation, data), &agent()).await
}
#[rstest]
#[tokio::test]
async fn message_effect_output_journal_and_final_lease_check_keep_their_order(mut scope: Scope) {
	let data = json!({"key":"id","content":"saved"});
	assert_eq!(
		run(&mut scope, "message", &data).await.unwrap(),
		json!({"sent":true})
	);
	assert_eq!(
		&scope.calls[4..],
		&[
			"task_resource",
			"workspace",
			"require:message.create",
			"apply",
			"output:message",
			"journal",
			"live"
		]
	);
	assert_eq!(
		scope.applied[0],
		json!({"effect":{"kind":"message","content":"saved","through":null},"task":Uuid::from_u128(2),"owner":qualified_agent("aidash://peer","agent","1"),"key":format!("scoped:{}:message:id",Uuid::from_u128(4)),"admission":Uuid::from_u128(1)})
	);
	assert_eq!(scope.journal[0]["result"], json!({"sent":true}));
	assert_eq!(scope.journal[0]["key"], json!("message:id"));
}
#[rstest]
#[tokio::test]
async fn exact_durable_replays_repeat_neither_effects_nor_fresh_lease_checks(mut scope: Scope) {
	let data = json!({"key":"id","content":"saved"});
	scope.previous = Some((
		commands::prepare("message", &data).unwrap().digest,
		json!({"saved":"original"}),
	));
	scope.task.status = TaskStatus::Completed;
	scope.task.owner = Some("other".into());
	scope.live = false;
	assert_eq!(
		run(&mut scope, "message", &data).await.unwrap(),
		json!({"saved":"original"})
	);
	assert_eq!(scope.calls.len(), 4);
	assert!(scope.applied.is_empty());
	assert!(scope.journal.is_empty());
}
#[rstest]
#[case::builtin("builtin:workspace_message")]
#[case::workspace("workspace")]
#[case::permission("require:message.create")]
#[case::effect("apply")]
#[case::output("output:message")]
#[case::journal("journal")]
#[case::lease("live")]
#[tokio::test]
async fn port_failures_stop_the_procedure_at_the_failed_boundary(
	mut scope: Scope,
	#[case] failure: &str,
) {
	scope.failure = Some(failure.into());
	assert!(
		matches!(run(&mut scope,"message",&json!({"key":"id","content":"saved"})).await,Err(Error::External(message)) if message==format!("fault:{failure}"))
	);
	assert_eq!(scope.calls.last().unwrap(), failure);
}
#[rstest]
#[tokio::test]
async fn lease_revocation_after_journaling_rejects_the_transaction_result(mut scope: Scope) {
	scope.live = false;
	assert!(matches!(
		run(
			&mut scope,
			"message",
			&json!({"key":"id","content":"saved"})
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(&scope.calls[scope.calls.len() - 2..], &["journal", "live"]);
}
#[rstest]
#[tokio::test]
async fn a_dependency_in_another_workspace_stops_child_creation_before_any_effect(
	mut scope: Scope,
) {
	let mut dependency = scope.child();
	dependency.workspace_id = Uuid::nil();
	let id = dependency.id;
	scope.additional.insert(id, dependency);
	let data = json!({"key":"id","task":{"title":"child","description":"","requirements":{},"dependencies":[id],"parent_id":scope.task.id}});
	assert!(matches!(
		run(&mut scope, "create_task", &data).await,
		Err(Error::Forbidden)
	));
	assert!(scope.applied.is_empty());
	assert!(scope.journal.is_empty());
}
#[rstest]
#[case::provenance(false, "aidash://home", false)]
#[case::foreign_target(true, "aidash://other", false)]
#[case::bound_child(true, "aidash://home", true)]
#[tokio::test]
async fn delegation_requires_both_grant_output_provenance_and_the_local_target(
	mut scope: Scope,
	#[case] created: bool,
	#[case] node: &str,
	#[case] allowed: bool,
) {
	let child = scope.child();
	let id = child.id;
	scope.additional.insert(id, child);
	scope.created = created;
	let data = json!({"key":"id","task_id":id,"node_id":node,"agent":{"id":"next","version":"2"}});
	let result = run(&mut scope, "delegate", &data).await;
	if allowed {
		assert!(result.is_ok());
		assert_eq!(
			scope.applied[0]["effect"],
			json!({"kind":"delegate","child":id,"agent":{"id":"next","version":"2"}})
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert!(scope.applied.is_empty());
	}
	assert!(scope.calls.contains(&"provenance".into()));
}
#[rstest]
#[case::terminal("run_message_terminal_transition",json!({"key":"id","revision":8,"status":"COMPLETED","through_seq":-1}),"invalid terminal sequence")]
#[case::complete("run_message_complete",json!({"key":"id","artifact":{"kind":"text","name":"a","content":"saved"},"through_seq":-1}),"invalid input sequence")]
#[tokio::test]
async fn negative_terminal_boundaries_fail_before_persistence(
	mut scope: Scope,
	#[case] operation: &str,
	#[case] data: Value,
	#[case] message: &str,
) {
	assert!(
		matches!(run(&mut scope,operation,&data).await,Err(Error::Invalid(error)) if error==message)
	);
	assert!(scope.applied.is_empty());
	assert!(scope.journal.is_empty());
}
#[rstest]
#[case::reserve("run_message_reserve", "reserve")]
#[case::commit("run_message_commit", "commit")]
#[case::deliver("run_message_delivery", "deliver")]
#[tokio::test]
async fn input_delivery_uses_run_and_workspace_authority_without_mutation_replay(
	mut scope: Scope,
	#[case] operation: &str,
	#[case] kind: &str,
) {
	let key = format!("human:{}:input", Uuid::from_u128(1));
	let data = json!({"key":key,"content":"input","input_seq":17});
	assert!(run(&mut scope, operation, &data).await.is_ok());
	assert_eq!(
		&scope.calls[2..],
		&[
			"task_resource",
			"require:run.message",
			"workspace",
			"require:message.create",
			"apply",
			"live"
		]
	);
	assert_eq!(scope.applied[0]["effect"]["kind"], json!(kind));
	assert_eq!(scope.applied[0]["effect"]["key"], json!(key));
	if kind == "commit" {
		assert_eq!(scope.applied[0]["effect"]["seq"], json!(17));
	} else {
		assert_eq!(scope.applied[0]["effect"]["node"], json!("aidash://peer"));
	}
	assert!(scope.journal.is_empty());
}
#[rstest]
#[tokio::test]
async fn empty_inputs_are_rejected_after_both_authority_checks(mut scope: Scope) {
	let data = json!({"key":format!("human:{}:input",Uuid::from_u128(1)),"content":""});
	assert!(
		matches!(run(&mut scope,"run_message_reserve",&data).await,Err(Error::Domain(aidash_domain::Error::Invalid(message))) if message=="run message must contain 1 to 64000 bytes")
	);
	assert_eq!(scope.calls.last().unwrap(), "require:message.create");
	assert!(scope.applied.is_empty());
}
#[rstest]
#[case::ack("run_message_ack", false)]
#[case::release("run_message_release", true)]
#[tokio::test]
async fn input_acknowledgement_and_release_keep_their_existing_effect_contract(
	mut scope: Scope,
	#[case] operation: &str,
	#[case] written: bool,
) {
	let keys = vec![format!("human:{}:input", Uuid::from_u128(1))];
	assert_eq!(
		run(&mut scope, operation, &json!({"keys":keys}))
			.await
			.unwrap(),
		json!({"acknowledged":true})
	);
	assert_eq!(!scope.applied.is_empty(), written);
	assert!(scope.journal.is_empty());
	assert_eq!(scope.calls.last().unwrap(), "live");
}
#[rstest]
#[case::foreign_run(vec![format!("human:{}:input",Uuid::nil())],true)]
#[case::too_many(vec![format!("human:{}:input",Uuid::from_u128(1));1025],false)]
#[tokio::test]
async fn release_validates_every_key_before_its_effect(
	mut scope: Scope,
	#[case] keys: Vec<String>,
	#[case] domain: bool,
) {
	let result = run(&mut scope, "run_message_release", &json!({"keys":keys})).await;
	if domain {
		assert!(
			matches!(result,Err(Error::Domain(aidash_domain::Error::Invalid(message))) if message=="invalid scoped run message key")
		);
	} else {
		assert!(matches!(result,Err(Error::Invalid(message)) if message=="too many input keys"));
	}
	assert!(scope.applied.is_empty());
}
#[rstest]
#[tokio::test]
async fn audit_effects_bind_the_saved_task_grant_and_admission_without_protected_bytes(
	mut scope: Scope,
) {
	let data = json!({"key":"id","kind":"remote.tool.completed","data":{"run_id":Uuid::from_u128(1),"call":{"id":"call","name":"tool","arguments":"protected"},"result":"protected"}});
	assert_eq!(
		run(&mut scope, "event", &data).await.unwrap(),
		json!({"recorded":true})
	);
	assert_eq!(
		scope.applied[0]["effect"],
		json!({"kind":"event","event_kind":"task.remote_tool_completed","data":{"task_id":Uuid::from_u128(2),"grant_id":Uuid::from_u128(4),"remote_run_id":Uuid::from_u128(1),"detail":{"call":{"id":"call","name":"tool"}}}})
	);
	assert_eq!(scope.journal.len(), 1);
}
#[rstest]
#[tokio::test]
async fn history_rechecks_disclosure_for_every_stored_message(mut scope: Scope) {
	scope.history = (10..12)
		.map(|id| Message {
			id: Uuid::from_u128(id),
			workspace_id: scope.task.workspace_id,
			sender: "human@aidash://peer".into(),
			content: "protected".into(),
			idempotency_key: None,
			created_at: chrono::Utc::now(),
		})
		.collect();
	assert_eq!(
		run(&mut scope, "run_message_history", &json!({"offset":7}))
			.await
			.unwrap(),
		json!([{"visible":true},{"visible":true}])
	);
	assert_eq!(
		&scope.calls[3..],
		&[
			"history",
			&format!("record:message:{}", Uuid::from_u128(10)),
			&format!("record:message:{}", Uuid::from_u128(11)),
			"live"
		]
	);
	assert!(scope.journal.is_empty());
}
#[rstest]
#[tokio::test]
async fn chunk_size_errors_follow_the_original_record_disclosure_check(mut scope: Scope) {
	let id = Uuid::from_u128(9);
	let data = json!({"kind":"artifact","id":id,"offset":0,"max_chars":16001});
	assert!(
		matches!(run(&mut scope,"workspace_record_chunk",&data).await,Err(Error::Invalid(message)) if message=="workspace chunk exceeds 16000 characters")
	);
	assert_eq!(
		scope.calls.last().unwrap(),
		&format!("record:artifact:{id}")
	);
}
#[rstest]
#[tokio::test]
async fn a_missing_output_identifier_rejects_the_effect_before_journaling(mut scope: Scope) {
	scope.output = None;
	assert!(
		matches!(run(&mut scope,"message",&json!({"key":"id","content":"saved"})).await,Err(Error::External(message)) if message=="remote output identifier missing")
	);
	assert!(scope.journal.is_empty());
	assert_eq!(scope.calls.last().unwrap(), "apply");
}

#[rstest]
#[tokio::test]
async fn claims_bind_the_complete_inspected_agent_definition(mut scope: Scope) {
	let definition = agent();
	let data = json!({"entry":definition,"revision":8});
	assert!(run(&mut scope, "claim", &data).await.is_ok());
	assert_eq!(
		scope.applied[0]["effect"],
		json!({"kind":"claim","revision":8,"agent":definition})
	);
}
#[rstest]
#[case::only_reference(json!({"id":"agent","version":"1"}))]
#[case::changed_config(json!({"id":"agent","version":"1","kind":"agent","name":{"en":"Fixture agent"},"description":{"en":"Fixture description"},"capabilities":[],"tags":[],"languages":[],"skills":[],"schema":{},"config":{"tools":["other-tool"]}}))]
#[tokio::test]
async fn a_matching_agent_id_and_version_do_not_authorize_an_incomplete_or_changed_claim_definition(
	mut scope: Scope,
	#[case] entry: Value,
) {
	assert!(matches!(
		run(&mut scope, "claim", &json!({"entry":entry,"revision":8})).await,
		Err(Error::Forbidden)
	));
	assert!(scope.applied.is_empty());
	assert!(scope.journal.is_empty());
}
