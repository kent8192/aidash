use super::*;
use aidash_domain::{
	RunControl, RunMetadata, RunPhase, TaskStatus, context::Context, policy::Resource,
	run_state::RawRun,
};
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
fn run(value: u128, visible: bool) -> RawRun {
	RawRun {
		metadata: RunMetadata {
			node: "brave-otter-k3m9q2xa".parse().unwrap(),
			id: id(value),
			task_id: id(2),
			workspace_id: id(1),
			home_node: "aidash://home".into(),
			agent_id: if visible { "saved-agent" } else { "hidden" }.into(),
			agent_version: "1".into(),
			phase: RunPhase::Ready,
			control: RunControl::Active,
			step: 0,
			revision: 3,
			observed_input_seq: 0,
			ledger_worker_ready: false,
			control_revision: 0,
			control_source: Default::default(),
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: Utc::now(),
		},
		context: serde_json::to_value(Context::default()).unwrap(),
		pending: json!({"state_version":1,"data":{},"recovery":{"retry":null}}),
	}
}
fn human(value: u128, visible: bool) -> HumanRequest {
	HumanRequest {
		id: id(value),
		workspace_id: id(1),
		run_id: id(3),
		kind: "question".into(),
		prompt: if visible { "saved-author" } else { "hidden" }.into(),
		response: None,
		answered_by: None,
		answer_by: None,
		created_at: Utc::now(),
	}
}
#[derive(Default)]
struct Scope {
	candidates: Vec<Uuid>,
	tasks: Vec<Task>,
	calls: Vec<Uuid>,
	humans: Vec<HumanRequest>,
	runs: Vec<RawRun>,
	run_reads: usize,
	deadline: Option<DateTime<Utc>>,
	listing_fails: bool,
	listing_called: bool,
	policy_fails: bool,
}
#[async_trait]
impl WorkspaceListingScope for Scope {
	async fn visible(&mut self, _: &str) -> Result<Vec<Uuid>> {
		if self.listing_fails {
			return Err(crate::Error::Forbidden);
		}
		Ok(vec![id(1)])
	}
	async fn task_page(&mut self, workspaces: &[Uuid], offset: u64) -> Result<TaskPage> {
		assert_eq!(workspaces, [id(1)]);
		assert_eq!(offset, 400);
		self.listing_called = true;
		Ok(TaskPage {
			tasks: vec![task(2)],
			next_offset: Some(9),
		})
	}
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
		Ok(self.runs.clone())
	}
	async fn pending_human_candidates(&mut self, _: Option<&str>) -> Result<Vec<HumanRequest>> {
		Ok(self.humans.clone())
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
	async fn run_visible(&mut self, row: &RunMetadata) -> Result<bool> {
		Ok(row.agent_id != "hidden")
	}
	async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>> {
		self.run_reads += 1;
		let mut result = run(id.as_u128(), true).inspect();
		if let Some(deadline) = self.deadline {
			result.state = Some(RunState::Waiting(Box::new(WaitingState::Human {
				request_id: self.humans[0].id,
				answer_by: Some(deadline),
				resume: aidash_domain::ResumeState::Ready(aidash_domain::ReadyState {}),
			})));
		}
		Ok(Some(result))
	}
	async fn human_resource(&mut self, row: &HumanRequest) -> Result<Resource> {
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "human".into(),
			id: row.id.to_string(),
			attributes: json!({}),
		})
	}
	async fn conversation_resource(&mut self, _: &Conversation) -> Result<Resource> {
		unreachable!()
	}
	async fn decide(&mut self, _: &Resource, _: &str) -> Result<bool> {
		if self.policy_fails {
			Err(crate::Error::Forbidden)
		} else {
			Ok(true)
		}
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
		candidates: (1..=49).map(id).chain(std::iter::once(id(99))).collect(),
		tasks: vec![other, repeated, denied, task(1003)],
		..Default::default()
	};
	let result = state(&mut scope, None).await.unwrap();
	assert_eq!(result.next_cursors.workspaces, Some(id(99).to_string()));
	assert_eq!(result.next_cursors.tasks, None);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|&&workspace| workspace == id(90))
			.count(),
		1
	);
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|&&workspace| workspace == id(1))
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
	assert_eq!(result.tasks.len(), 3);
}

#[tokio::test]
async fn pending_requests_of_an_older_run_remain_visible_and_share_its_decision() {
	let deadline: DateTime<Utc> = "2026-10-19T00:00:00Z".parse().unwrap();
	let mut scope = Scope {
		humans: vec![human(20, true), human(21, true)],
		deadline: Some(deadline),
		..Default::default()
	};
	let output = state(&mut scope, None).await.unwrap();
	assert!(output.runs.is_empty());
	assert_eq!(output.human_requests.len(), 2);
	assert_eq!(output.human_requests[0].answer_by, Some(deadline));
	assert_eq!(output.human_requests[1].answer_by, None);
	assert_eq!(scope.run_reads, 1);
	assert_eq!(scope.calls, [id(1)]);
	assert_eq!(output.workspaces[0].id, id(1));
	assert!(output.next_cursors.human_requests.is_none());
}

#[tokio::test]
async fn policy_failure_does_not_return_partial_state() {
	let mut scope = Scope {
		humans: vec![human(20, true)],
		policy_fails: true,
		..Default::default()
	};
	assert!(matches!(
		state(&mut scope, None).await,
		Err(crate::Error::Forbidden)
	));
}

#[tokio::test]
async fn hidden_task_candidates_still_advance_the_page() {
	let mut scope = Scope {
		tasks: (1..=50)
			.map(|n| {
				let mut row = task(n);
				row.workspace_id = id(99);
				row
			})
			.collect(),
		..Default::default()
	};
	let output = state(&mut scope, None).await.unwrap();
	assert!(output.tasks.is_empty());
	assert_eq!(output.next_cursors.tasks, Some(id(50).to_string()));
	assert_eq!(scope.calls, [id(99)]);
}

#[tokio::test]
async fn standalone_task_page_preserves_cursor_and_stops_on_authority_failure() {
	let mut scope = Scope::default();
	let page = task_page(&mut scope, 400).await.unwrap();
	assert_eq!(page.tasks[0].id, id(2));
	assert_eq!(page.next_offset, Some(9));
	assert!(scope.listing_called);
	let mut denied = Scope {
		listing_fails: true,
		..Default::default()
	};
	assert!(matches!(
		task_page(&mut denied, 400).await,
		Err(crate::Error::Forbidden)
	));
	assert!(!denied.listing_called);
}

#[tokio::test]
async fn malformed_visible_run_does_not_hide_its_neighbors_and_hidden_run_is_omitted() {
	let mut malformed = run(8, true);
	malformed.pending = json!({"state_version":999});
	let mut hidden = run(9, false);
	hidden.pending = json!({});
	let mut scope = Scope {
		runs: vec![malformed, hidden, run(3, true)],
		..Default::default()
	};
	let output = state(&mut scope, None).await.unwrap();
	assert_eq!(
		output.runs.iter().map(|row| row.id).collect::<Vec<_>>(),
		[id(8), id(3)]
	);
	assert!(output.runs[0].state_error.is_some());
	assert!(output.runs[1].state_error.is_none());
}
