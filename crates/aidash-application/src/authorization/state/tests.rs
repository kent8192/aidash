use super::*;
use aidash_domain::{RunMetadata, TaskStatus, policy::Resource, run_state::RawRun};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
fn workspace() -> Workspace {
	Workspace {
		scratch_space_id: uuid::Uuid::nil(),
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		id: id(1),
		title: "saved workspace".into(),
		goal: "goal".into(),
		state: json!({}),
		revision: 4,
		created_at: Utc::now(),
	}
}
fn task(value: u128) -> Task {
	Task {
		id: id(value),
		workspace_id: id(1),
		title: "task".into(),
		description: "description".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: "saved-author".into(),
		dependencies: vec![],
		parent_id: None,
		revision: 1,
		created_at: Utc::now(),
	}
}
#[derive(Default)]
struct Scope {
	candidates: Vec<Uuid>,
	tasks: Vec<Task>,
	calls: Vec<Uuid>,
}
#[async_trait]
impl WorkspaceStateScope for Scope {
	async fn allowed(&mut self, id: Uuid, action: &str) -> Result<bool> {
		assert_eq!(action, "workspace.read");
		self.calls.push(id);
		Ok(id != Uuid::from_u128(99))
	}
	async fn registry(&mut self) -> Result<Vec<Entry>> {
		Ok(vec![])
	}
	async fn workspace_candidates(
		&mut self,
		_: Option<&str>,
		_: Option<Uuid>,
	) -> Result<Vec<Uuid>> {
		Ok(self.candidates.clone())
	}
	async fn workspace_rows(&mut self, ids: &[Uuid]) -> Result<Vec<Workspace>> {
		Ok(ids
			.iter()
			.map(|id| Workspace {
				id: *id,
				..workspace()
			})
			.collect())
	}
	async fn task_candidates(&mut self, _: Option<&str>) -> Result<Vec<Task>> {
		Ok(self.tasks.clone())
	}
	async fn artifact_candidates(&mut self, _: Option<&str>) -> Result<Vec<Artifact>> {
		Ok(vec![])
	}
	async fn run_candidates(&mut self, _: Option<&str>) -> Result<Vec<RawRun>> {
		Ok(vec![])
	}
	async fn pending_human_candidates(&mut self, _: Option<&str>) -> Result<Vec<HumanRequest>> {
		Ok(vec![])
	}
	async fn conversation_candidates(&mut self, _: Option<&str>) -> Result<Vec<Conversation>> {
		Ok(vec![])
	}
	async fn latest_visible_events(&mut self, _: Option<&str>) -> Result<Vec<Event>> {
		Ok(vec![])
	}
	async fn task_visible(&mut self, _: &Task) -> Result<bool> {
		Ok(true)
	}
	async fn artifact_visible(&mut self, _: &Artifact) -> Result<bool> {
		unreachable!()
	}
	async fn run_visible(&mut self, _: &RunMetadata) -> Result<bool> {
		unreachable!()
	}
	async fn visible_run(&mut self, _: Uuid) -> Result<Option<RunInspection>> {
		unreachable!()
	}
	async fn human_resource(&mut self, _: &HumanRequest) -> Result<Resource> {
		unreachable!()
	}
	async fn conversation_resource(&mut self, _: &Conversation) -> Result<Resource> {
		unreachable!()
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		unreachable!()
	}
}
#[tokio::test]
async fn full_candidate_window_advances_even_when_denied_and_unions_item_workspaces() {
	let mut other = task(1000);
	other.workspace_id = id(90);
	let mut repeated = task(1001);
	repeated.workspace_id = id(90);
	let mut denied = task(1002);
	denied.workspace_id = id(99);
	let mut scope = Scope {
		candidates: (1..=50).map(id).collect(),
		tasks: vec![other, repeated, denied],
		..Default::default()
	};
	let result = state(&mut scope, None).await.unwrap();
	assert_eq!(result.next_cursors.workspaces, Some(id(50).to_string()));
	assert_eq!(result.next_cursors.tasks, None);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|&&workspace| workspace == id(90))
			.count(),
		1
	);
	assert!(
		result
			.workspaces
			.iter()
			.any(|workspace| workspace.id == id(90))
	);
	assert!(
		!result
			.workspaces
			.iter()
			.any(|workspace| workspace.id == id(99))
	);
	assert_eq!(result.tasks.len(), 2);
}
