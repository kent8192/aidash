use super::*;
use aidash_domain::identity::commands::Binding;
use async_trait::async_trait;
use rstest::{fixture, rstest};
struct Scope {
	binding: Option<Binding>,
	task: Task,
	previous: Option<(String, Value)>,
	calls: Vec<String>,
	failure: Option<String>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		binding: Some(Binding {
			admission_id: Uuid::from_u128(1),
			task_id: Uuid::from_u128(2),
		}),
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
		previous: None,
		calls: vec![],
		failure: None,
	}
}
impl Scope {
	fn call(&mut self, name: &str) -> Result<()> {
		self.calls.push(name.into());
		if self.failure.as_deref() == Some(name) {
			return Err(Error::External(format!("fault:{name}")));
		}
		Ok(())
	}
}
#[async_trait]
impl RemoteCommandScope for Scope {
	async fn binding(&mut self, _: Uuid) -> Result<Option<Binding>> {
		self.call("binding")?;
		Ok(self.binding.as_ref().map(|bound| Binding {
			admission_id: bound.admission_id,
			task_id: bound.task_id,
		}))
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		assert_eq!(id, self.task.id);
		self.call("task")?;
		Ok(self.task.clone())
	}
	async fn previous(&mut self, _: Uuid, key: &str) -> Result<Option<(String, Value)>> {
		assert_eq!(key, "message:id");
		self.call("previous")?;
		Ok(self.previous.clone())
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.call("task_resource")?;
		Ok(Resource {
			tenant: "source".into(),
			kind: "task".into(),
			id: task.id.to_string(),
			attributes: json!({}),
		})
	}
	async fn require_builtin(&mut self, tool: &str) -> Result<()> {
		self.call(&format!("builtin:{tool}"))
	}
}
fn input(data: &Value) -> Command<'_> {
	Command {
		grant_id: Uuid::from_u128(4),
		admission_id: Uuid::from_u128(1),
		operation: "message",
		data,
		node: "aidash://peer",
		agent: "agent",
		version: "1",
	}
}
#[rstest]
#[tokio::test]
async fn fresh_commands_authorize_the_saved_task_and_builtin_in_the_existing_order(
	mut scope: Scope,
) {
	let data = json!({"key":"id","content":"saved"});
	let Admission::Ready(prepared) = prepare(&mut scope, input(&data)).await.unwrap() else {
		panic!("fresh command must be prepared")
	};
	assert_eq!(
		scope.calls,
		vec![
			"binding",
			"task",
			"builtin:workspace_message",
			"previous",
			"task_resource"
		]
	);
	assert_eq!(prepared.task.id, Uuid::from_u128(2));
	assert_eq!(
		prepared.owner,
		qualified_agent("aidash://peer", "agent", "1")
	);
	assert_eq!(
		prepared.key,
		format!("scoped:{}:message:id", Uuid::from_u128(4))
	);
}
#[rstest]
#[tokio::test]
async fn an_exact_replay_survives_terminal_state_and_owner_changes_without_repeating_effect_checks(
	mut scope: Scope,
) {
	let data = json!({"key":"id","content":"saved"});
	scope.previous = Some((
		commands::prepare("message", &data).unwrap().digest,
		json!({"saved":"result"}),
	));
	scope.task.status = TaskStatus::Completed;
	scope.task.owner = Some("other".into());
	let Admission::Replay(saved) = prepare(&mut scope, input(&data)).await.unwrap() else {
		panic!("saved command must replay")
	};
	assert_eq!(saved, json!({"saved":"result"}));
	assert_eq!(
		scope.calls,
		vec!["binding", "task", "builtin:workspace_message", "previous"]
	);
}
#[rstest]
#[tokio::test]
async fn a_replay_key_cannot_be_rebound_to_changed_input(mut scope: Scope) {
	let data = json!({"key":"id","content":"changed"});
	scope.previous = Some((
		commands::prepare("message", &json!({"key":"id","content":"saved"}))
			.unwrap()
			.digest,
		json!({}),
	));
	assert!(
		matches!(prepare(&mut scope,input(&data)).await,Err(Error::Conflict(message)) if message=="remote command key binds different input")
	);
	assert_eq!(
		scope.calls,
		vec!["binding", "task", "builtin:workspace_message", "previous"]
	);
}
#[rstest]
#[case::missing("missing")]
#[case::admission("admission")]
#[case::run("run")]
#[tokio::test]
async fn binding_mismatches_fail_before_task_reads(mut scope: Scope, #[case] change: &str) {
	let mut data = json!({"key":"id"});
	match change {
		"missing" => scope.binding = None,
		"admission" => scope.binding.as_mut().unwrap().admission_id = Uuid::nil(),
		"run" => data["run_id"] = json!(Uuid::nil()),
		_ => unreachable!(),
	}
	assert!(matches!(
		prepare(&mut scope, input(&data)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["binding"]);
}
#[rstest]
#[case::owner("owner")]
#[case::terminal("terminal")]
#[tokio::test]
async fn fresh_mutations_cannot_replace_an_owner_or_change_terminal_tasks(
	mut scope: Scope,
	#[case] change: &str,
) {
	let data = json!({"key":"id"});
	if change == "owner" {
		scope.task.owner = Some("other".into());
	} else {
		scope.task.status = TaskStatus::Completed;
	}
	let result = prepare(&mut scope, input(&data)).await;
	if change == "owner" {
		assert!(matches!(result, Err(Error::Forbidden)));
	} else {
		assert!(
			matches!(result,Err(Error::Conflict(message)) if message=="remote task is terminal")
		);
	}
	assert_eq!(
		scope.calls,
		vec!["binding", "task", "builtin:workspace_message", "previous"]
	);
}
#[rstest]
#[case::binding("binding")]
#[case::task("task")]
#[case::previous("previous")]
#[case::resource("task_resource")]
#[case::builtin("builtin:workspace_message")]
#[tokio::test]
async fn failed_ports_preserve_the_error_and_stop_admission_at_that_boundary(
	mut scope: Scope,
	#[case] failure: &str,
) {
	let data = json!({"key":"id"});
	scope.failure = Some(failure.into());
	assert!(
		matches!(prepare(&mut scope,input(&data)).await,Err(Error::External(message)) if message==format!("fault:{failure}"))
	);
	assert_eq!(scope.calls.last().unwrap(), failure);
}

#[rstest]
#[tokio::test]
async fn revoked_builtin_stops_durable_replay_before_reading_saved_result(mut scope: Scope) {
	let data = json!({"key":"id","content":"saved"});
	scope.previous = Some((
		commands::prepare("message", &data).unwrap().digest,
		json!({"saved":"result"}),
	));
	scope.failure = Some("builtin:workspace_message".into());
	assert!(matches!(
		prepare(&mut scope, input(&data)).await,
		Err(Error::External(_))
	));
	assert_eq!(
		scope.calls,
		vec!["binding", "task", "builtin:workspace_message"]
	);
}
