use super::*;
use aidash_domain::{identity::commands::Binding, policy::Resource};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

struct Scope {
	binding: Option<Binding>,
	task: Task,
	calls: Vec<String>,
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
			thread_id: Uuid::from_u128(3),
			title: String::new(),
			description: String::new(),
			status: TaskStatus::Claimed,
			requirements: json!({}),
			owner: Some(aidash_domain::agent_subject("aidash://peer", "agent", "1")),
			created_by: "root".into(),
			dependencies: vec![],
			parent_id: None,
			started_by_duty: None,
			revision: 8,
			created_at: chrono::Utc::now(),
		},
		calls: vec![],
	}
}
#[async_trait]
impl RemoteCommandScope for Scope {
	async fn binding(&mut self, _: Uuid) -> Result<Option<Binding>> {
		self.calls.push("binding".into());
		Ok(self.binding.as_ref().map(|bound| Binding {
			admission_id: bound.admission_id,
			task_id: bound.task_id,
		}))
	}
	async fn task(&mut self, _: Uuid) -> Result<Task> {
		self.calls.push("task".into());
		Ok(self.task.clone())
	}
	async fn previous(&mut self, _: Uuid, _: &str) -> Result<Option<(String, Value)>> {
		panic!("a delivery has its own replay: the delivery id")
	}
	async fn task_resource(&mut self, _: &Task) -> Result<Resource> {
		panic!("a delivery acts on the Area and the Thread, not the task resource")
	}
	async fn require_builtin(&mut self, tool: &str) -> Result<()> {
		self.calls.push(format!("builtin:{tool}"));
		Ok(())
	}
}
fn who(admission: u128) -> Identity<'static> {
	Identity {
		grant_id: Uuid::from_u128(4),
		admission_id: Uuid::from_u128(admission),
		node: "aidash://peer",
		agent: "agent",
		version: "1",
		request: None,
	}
}
fn message() -> Purpose {
	Purpose::Message {
		content: "done".into(),
		descriptions: Default::default(),
	}
}

#[rstest]
#[tokio::test]
async fn an_admitted_delivery_checks_the_binding_then_the_tool_then_the_task(mut scope: Scope) {
	let task = verify(&mut scope, &who(1), "file_share").await.unwrap();
	assert_eq!(task.id, Uuid::from_u128(2));
	assert_eq!(scope.calls, ["binding", "builtin:file_share", "task"]);
}
#[rstest]
#[tokio::test]
async fn a_message_with_files_is_checked_as_the_message_tool(mut scope: Scope) {
	verify(&mut scope, &who(1), message().tool()).await.unwrap();
	assert!(scope.calls.contains(&"builtin:thread_message".to_string()));
	assert!(!scope.calls.contains(&"builtin:file_share".to_string()));
}
#[rstest]
#[tokio::test]
async fn a_grant_without_a_live_binding_is_refused_before_anything_else(mut scope: Scope) {
	scope.binding = None;
	assert!(matches!(
		verify(&mut scope, &who(1), "file_share").await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, ["binding"]);
}
#[rstest]
#[tokio::test]
async fn another_runs_admission_is_refused(mut scope: Scope) {
	assert!(matches!(
		verify(&mut scope, &who(9), "file_share").await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		["binding"],
		"no tool or task is read for a stranger"
	);
}
#[rstest]
#[tokio::test]
async fn a_task_another_agent_owns_is_refused(mut scope: Scope) {
	scope.task.owner = Some(aidash_domain::agent_subject("aidash://peer", "other", "1"));
	assert!(matches!(
		verify(&mut scope, &who(1), "file_share").await,
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case::completed(TaskStatus::Completed)]
#[case::failed(TaskStatus::Failed)]
#[case::cancelled(TaskStatus::Cancelled)]
#[case::abandoned(TaskStatus::Abandoned)]
#[tokio::test]
async fn a_finished_task_takes_no_delivery(mut scope: Scope, #[case] status: TaskStatus) {
	scope.task.status = status;
	assert!(matches!(
		verify(&mut scope, &who(1), "file_share").await,
		Err(Error::Conflict(_))
	));
}
#[rstest]
fn the_manifest_is_held_to_the_deployments_budgets() {
	let share = Limits {
		files: 2,
		file_bytes: 10,
		bytes: 15,
	};
	let file = |path: &str, size| File {
		path: path.into(),
		digest: "b".repeat(64),
		size,
		media_type: "text/plain".into(),
	};
	assert_eq!(
		check_manifest(&Purpose::FileShare, &[file("a", 10), file("b", 5)], share).unwrap(),
		15
	);
	assert!(check_manifest(&Purpose::FileShare, &[file("a", 11)], share).is_err());
	assert!(check_manifest(&Purpose::FileShare, &[file("a", 10), file("b", 10)], share).is_err());
	assert!(
		check_manifest(
			&Purpose::FileShare,
			&[file("a", 1), file("b", 1), file("c", 1)],
			share
		)
		.is_err()
	);
}
#[rstest]
fn only_the_home_may_receive() {
	assert!(require_home("aidash://home/node", "aidash://home/node").is_ok());
	assert!(matches!(
		require_home("aidash://home/node", "aidash://other/node"),
		Err(Error::Forbidden)
	));
	assert!(matches!(
		require_home("aidash://home/node", "aidash://home/another"),
		Err(Error::Forbidden)
	));
}
fn requesting(tools: &[&str]) -> FederationToolRequest {
	use aidash_domain::federation::tool_lists::{ToolList, ToolPattern};
	FederationToolRequest {
		tools: ToolList::new(tools.iter().map(|tool| ToolPattern::new(*tool).unwrap())).unwrap(),
		hosts: vec![],
	}
}
#[rstest]
#[case::share("file_share")]
#[case::message("thread_message")]
#[tokio::test]
async fn a_delivery_the_homes_own_request_does_not_name_is_refused_before_the_binding(
	mut scope: Scope,
	#[case] tool: &str,
) {
	let request = requesting(&["task_create"]);
	let mut identity = who(1);
	identity.request = Some(&request);
	assert!(matches!(
		verify(&mut scope, &identity, tool).await,
		Err(Error::Invalid(message)) if message == aidash_domain::federation::tool_lists::UNAVAILABLE_TOOL
	));
	assert_eq!(scope.calls, ["binding"], "neither Binding nor task is read");
}
#[rstest]
#[tokio::test]
async fn a_delivery_a_run_only_answer_allows_is_admitted(mut scope: Scope) {
	use aidash_domain::federation::tool_lists::{ToolList, ToolPattern};
	let run_only = ToolList::new([ToolPattern::new("file_share").unwrap()]).unwrap();
	let request = requesting(&["thread_message"])
		.with_run_tools(&run_only)
		.unwrap();
	let mut identity = who(1);
	identity.request = Some(&request);
	verify(&mut scope, &identity, "file_share").await.unwrap();
	assert_eq!(scope.calls, ["binding", "builtin:file_share", "task"]);
}
#[rstest]
#[case::plain_key_only(&["thread_message"], false)]
#[case::files_key_too(&["thread_message", "thread_message:files"], true)]
#[case::pattern(&["thread_message*"], true)]
#[case::pinned_name(&["workspace_message", "workspace_message:files"], true)]
#[tokio::test]
async fn a_message_delivery_needs_the_files_key_in_the_homes_request(
	mut scope: Scope,
	#[case] tools: &[&str],
	#[case] admitted: bool,
) {
	let request = requesting(tools);
	let mut identity = who(1);
	identity.request = Some(&request);
	let result = verify(&mut scope, &identity, message().tool()).await;
	assert_eq!(result.is_ok(), admitted, "{result:?}");
}
#[rstest]
#[tokio::test]
async fn a_delivery_named_by_a_pinned_workspace_tool_is_checked_as_its_thread_tool(
	mut scope: Scope,
) {
	// A receiving Node whose Agent pins `workspace_message` (#1008).
	let request = requesting(&["thread_message"]);
	let mut identity = who(1);
	identity.request = Some(&request);
	// The plain key alone does not admit a message with files under either name.
	assert!(
		verify(&mut scope, &identity, "workspace_message")
			.await
			.is_err()
	);
	let request = requesting(&["thread_message*"]);
	identity.request = Some(&request);
	scope.calls.clear();
	verify(&mut scope, &identity, "workspace_message")
		.await
		.unwrap();
	assert_eq!(scope.calls, ["binding", "builtin:thread_message", "task"]);
}
