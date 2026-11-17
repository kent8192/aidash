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
			workspace_id: Uuid::from_u128(3),
			title: String::new(),
			description: String::new(),
			status: TaskStatus::Claimed,
			requirements: json!({}),
			owner: Some(aidash_domain::agent_subject("aidash://peer", "agent", "1")),
			created_by: "root".into(),
			dependencies: vec![],
			parent_id: None,
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
		panic!("a delivery acts on the Area and the Workspace, not the task resource")
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
	assert!(
		scope
			.calls
			.contains(&"builtin:workspace_message".to_string())
	);
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
