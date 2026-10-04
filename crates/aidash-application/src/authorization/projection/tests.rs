use super::*;
use crate::Error;
use aidash_domain::{Artifact, TaskStatus, Workspace, policy::Resource};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
use serde_json::json;
use std::collections::BTreeMap;
fn workspace() -> Workspace {
	Workspace {
		id: Uuid::from_u128(1),
		title: "workspace".into(),
		goal: "goal".into(),
		state: json!({}),
		revision: 1,
		created_at: Utc::now(),
	}
}
fn task(id: usize, visible: bool) -> Task {
	Task {
		id: Uuid::from_u128(id as u128),
		workspace_id: Uuid::from_u128(1),
		title: "task".into(),
		description: "description".into(),
		status: TaskStatus::Open,
		requirements: json!({}),
		owner: None,
		created_by: if visible { "visible" } else { "hidden" }.into(),
		dependencies: vec![],
		parent_id: None,
		revision: 1,
		created_at: Utc::now(),
	}
}
fn message(id: usize, visible: bool) -> Message {
	Message {
		id: Uuid::from_u128(id as u128),
		workspace_id: Uuid::from_u128(1),
		sender: if visible { "visible" } else { "hidden" }.into(),
		content: "saved content".into(),
		idempotency_key: None,
		created_at: Utc::now(),
	}
}
fn event(sequence: i64, visible: bool) -> Event {
	Event {
		sequence,
		id: Uuid::from_u128(sequence as u128),
		node_id: "aidash://local".into(),
		workspace_id: Some(Uuid::from_u128(1)),
		kind: "task.created".into(),
		data: json!({"visible":visible}),
		created_at: Utc::now(),
	}
}
fn artifact(id: usize, visible: bool) -> Artifact {
	Artifact {
		id: Uuid::from_u128(id as u128),
		workspace_id: Uuid::from_u128(1),
		task_id: Uuid::from_u128(2),
		kind: "text".into(),
		name: "artifact".into(),
		content: json!("immutable content"),
		created_by: if visible { "visible" } else { "hidden" }.into(),
		idempotency_key: "artifact".into(),
		created_at: Utc::now(),
	}
}
struct Scope {
	tasks: Vec<Task>,
	events: Vec<Event>,
	messages: Vec<Message>,
	artifacts: Vec<Artifact>,
	calls: Vec<&'static str>,
	task_offsets: Vec<u64>,
	message_offsets: Vec<u64>,
	event_pages: Vec<(i64, usize, bool)>,
	message_visits: BTreeMap<Uuid, usize>,
	second_message_denied: bool,
	require_allowed: bool,
	events_allowed: bool,
	tracked: Vec<WorkspaceSnapshot>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		tasks: vec![],
		events: vec![],
		messages: vec![],
		artifacts: vec![],
		calls: vec![],
		task_offsets: vec![],
		message_offsets: vec![],
		event_pages: vec![],
		message_visits: BTreeMap::new(),
		second_message_denied: false,
		require_allowed: true,
		events_allowed: true,
		tracked: vec![],
		fail: None,
	}
}
impl Scope {
	fn touch(&mut self, call: &'static str) -> Result<()> {
		self.calls.push(call);
		if self.fail == Some(call) {
			Err(Error::Port(Box::new(std::io::Error::other(call))))
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl WorkspaceProjection for Scope {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		assert_eq!(id, Uuid::from_u128(1));
		self.touch("workspace")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: id.to_string(),
			attributes: json!({"owner":"saved-owner"}),
		})
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(resource.attributes["owner"], "saved-owner");
		assert_eq!(action, "workspace.read");
		self.touch("require")?;
		if self.require_allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		assert_eq!(action, "workspace.events");
		self.touch("decide")?;
		Ok(self.events_allowed)
	}
	async fn task_visible(&mut self, row: &Task) -> Result<bool> {
		self.touch("task_visible")?;
		Ok(row.created_by == "visible")
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		self.touch("artifact_visible")?;
		Ok(row.created_by == "visible")
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		self.touch("message_visible")?;
		let visits = self.message_visits.entry(row.id).or_default();
		*visits += 1;
		Ok(row.sender == "visible" && !(self.second_message_denied && *visits > 1))
	}
	async fn event_visible(&mut self, row: &Event) -> Result<bool> {
		self.touch("event_visible")?;
		Ok(row.data["visible"] == true)
	}
	async fn task_rows(&mut self, workspaces: &[Uuid], cursor: u64) -> Result<Vec<Task>> {
		assert_eq!(workspaces, &[Uuid::from_u128(1)]);
		self.touch("task_rows")?;
		self.task_offsets.push(cursor);
		Ok(self
			.tasks
			.iter()
			.skip(cursor as usize)
			.take(500)
			.cloned()
			.collect())
	}
	async fn event_rows(
		&mut self,
		workspaces: &[Uuid],
		include_marketplace: bool,
		cursor: i64,
		page_size: usize,
	) -> Result<Vec<Event>> {
		assert_eq!(workspaces, &[Uuid::from_u128(1)]);
		self.touch("event_rows")?;
		self.event_pages
			.push((cursor, page_size, include_marketplace));
		Ok(self
			.events
			.iter()
			.filter(|row| row.sequence < cursor)
			.take(page_size)
			.cloned()
			.collect())
	}
	async fn message_rows(&mut self, id: Uuid, offset: u64) -> Result<Vec<Message>> {
		assert_eq!(id, Uuid::from_u128(1));
		self.touch("message_rows")?;
		self.message_offsets.push(offset);
		Ok(self
			.messages
			.iter()
			.skip(offset as usize)
			.take(100)
			.cloned()
			.collect())
	}
	async fn workspace_row(&mut self, id: Uuid) -> Result<Workspace> {
		assert_eq!(id, Uuid::from_u128(1));
		self.touch("workspace_row")?;
		Ok(workspace())
	}
	async fn workspace_tasks(&mut self, id: Uuid) -> Result<Vec<Task>> {
		assert_eq!(id, Uuid::from_u128(1));
		self.touch("workspace_tasks")?;
		Ok(self.tasks.clone())
	}
	async fn workspace_artifacts(&mut self, id: Uuid) -> Result<Vec<Artifact>> {
		assert_eq!(id, Uuid::from_u128(1));
		self.touch("workspace_artifacts")?;
		Ok(self.artifacts.clone())
	}
	async fn track(&mut self, snapshot: &WorkspaceSnapshot) -> Result<()> {
		self.touch("track")?;
		self.tracked.push(snapshot.clone());
		Ok(())
	}
}
#[rstest]
#[case::empty(0, None)]
#[case::short(499, None)]
#[case::full(500, Some(500))]
#[case::more(501, Some(500))]
#[tokio::test]
async fn task_page_keeps_the_existing_full_page_cursor(
	mut scope: Scope,
	#[case] rows: usize,
	#[case] next: Option<u64>,
) {
	scope.tasks = (1..=rows).map(|id| task(id, true)).collect();
	let page = task_page(&mut scope, &[Uuid::from_u128(1)], 0)
		.await
		.unwrap();
	assert_eq!(page.tasks.len(), rows.min(500));
	assert_eq!(page.next_offset, next);
	assert_eq!(scope.task_offsets, vec![0]);
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn hidden_tasks_advance_offsets_until_five_hundred_visible_rows(mut scope: Scope) {
	scope.tasks = (0..1400).map(|id| task(id, id >= 850)).collect();
	let page = task_page(&mut scope, &[Uuid::from_u128(1)], 50)
		.await
		.unwrap();
	assert_eq!(page.tasks.len(), 500);
	assert_eq!(page.tasks[0].id, Uuid::from_u128(850));
	assert_eq!(page.tasks[499].id, Uuid::from_u128(1349));
	assert_eq!(page.next_offset, Some(1350));
	assert_eq!(scope.task_offsets, vec![50, 550, 1050]);
}
#[rstest]
#[tokio::test]
async fn an_entire_hidden_task_page_does_not_hide_later_visible_rows(mut scope: Scope) {
	scope.tasks = (0..501).map(|id| task(id, id == 500)).collect();
	let page = task_page(&mut scope, &[Uuid::from_u128(1)], 0)
		.await
		.unwrap();
	assert_eq!(
		page.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
		vec![Uuid::from_u128(500)]
	);
	assert_eq!(page.next_offset, None);
	assert_eq!(scope.task_offsets, vec![0, 500]);
}
#[rstest]
#[tokio::test]
async fn latest_events_are_disclosed_oldest_first_after_hidden_rows(mut scope: Scope) {
	scope.events = (1..=500).rev().map(|seq| event(seq, seq <= 250)).collect();
	let rows = latest_visible_events(&mut scope, &[Uuid::from_u128(1)], true)
		.await
		.unwrap();
	assert_eq!(
		rows.iter().map(|row| row.sequence).collect::<Vec<_>>(),
		(151..=250).collect::<Vec<_>>()
	);
	assert_eq!(
		scope.event_pages,
		vec![
			(i64::MAX, 100, true),
			(401, 100, true),
			(301, 100, true),
			(201, 100, true)
		]
	);
}
#[rstest]
#[tokio::test]
async fn latest_event_scan_counts_hidden_rows_and_reduces_the_last_batch_to_the_budget(
	mut scope: Scope,
) {
	scope.events = (1..=5000).rev().map(|seq| event(seq, false)).collect();
	assert!(
		latest_visible_events(&mut scope, &[Uuid::from_u128(1)], false)
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(scope.event_pages.len(), 41);
	assert_eq!(scope.event_pages.last(), Some(&(1001, 96, false)));
	assert_eq!(
		scope
			.calls
			.iter()
			.filter(|call| **call == "event_visible")
			.count(),
		4096
	);
}
#[rstest]
#[case::empty(0)]
#[case::short(17)]
#[case::full(100)]
#[case::over(101)]
#[tokio::test]
async fn latest_visible_events_stop_at_the_first_exhausted_or_complete_batch(
	mut scope: Scope,
	#[case] rows: i64,
) {
	scope.events = (1..=rows).rev().map(|seq| event(seq, true)).collect();
	let result = latest_visible_events(&mut scope, &[Uuid::from_u128(1)], false)
		.await
		.unwrap();
	assert_eq!(result.len(), (rows as usize).min(100));
	assert_eq!(scope.event_pages.len(), 1);
}
#[rstest]
#[tokio::test]
async fn latest_messages_cross_hidden_full_pages_and_keep_oldest_first_output(mut scope: Scope) {
	scope.messages = (0..350).map(|id| message(id, id >= 150)).collect();
	let result = latest_visible_messages(&mut scope, Uuid::from_u128(1))
		.await
		.unwrap();
	assert_eq!(
		result.iter().map(|row| row.id).collect::<Vec<_>>(),
		(150..250).rev().map(Uuid::from_u128).collect::<Vec<_>>()
	);
	assert_eq!(scope.message_offsets, vec![0, 100, 200]);
}
#[rstest]
#[case::empty(0)]
#[case::short(99)]
#[case::full(100)]
#[case::hidden_tail(201)]
#[tokio::test]
async fn hidden_message_rows_are_scanned_until_repository_exhaustion(
	mut scope: Scope,
	#[case] count: usize,
) {
	scope.messages = (0..count).map(|id| message(id, false)).collect();
	assert!(
		latest_visible_messages(&mut scope, Uuid::from_u128(1))
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		scope.message_offsets,
		(0..=count / 100)
			.map(|page| (page * 100) as u64)
			.collect::<Vec<_>>()
	);
}
#[rstest]
#[tokio::test]
async fn snapshot_rechecks_message_visibility_and_tracks_only_final_disclosure(mut scope: Scope) {
	scope.tasks = vec![task(3, true), task(2, false), task(1, true)];
	scope.artifacts = vec![artifact(8, true), artifact(9, false)];
	scope.messages = vec![message(12, true), message(11, false)];
	scope.events = vec![event(9, true), event(8, false)];
	scope.second_message_denied = true;
	let result = snapshot(&mut scope, Uuid::from_u128(1)).await.unwrap();
	assert_eq!(
		result.tasks.iter().map(|row| row.id).collect::<Vec<_>>(),
		vec![Uuid::from_u128(3), Uuid::from_u128(1)]
	);
	assert_eq!(result.artifacts[0].content, json!("immutable content"));
	assert_eq!(result.artifacts.len(), 1);
	assert!(result.messages.is_empty());
	assert_eq!(
		result
			.events
			.iter()
			.map(|row| row.sequence)
			.collect::<Vec<_>>(),
		vec![9]
	);
	assert_eq!(scope.message_visits[&Uuid::from_u128(12)], 2);
	assert_eq!(scope.tracked.len(), 1);
	assert!(scope.tracked[0].messages.is_empty());
	assert_eq!(scope.calls.last(), Some(&"track"));
	let calls: Vec<_> = scope
		.calls
		.iter()
		.copied()
		.filter(|call| !call.ends_with("_visible") && *call != "track")
		.collect();
	assert_eq!(
		calls,
		vec![
			"workspace",
			"require",
			"decide",
			"event_rows",
			"workspace_row",
			"workspace_tasks",
			"workspace_artifacts",
			"message_rows"
		]
	);
}
#[rstest]
#[tokio::test]
async fn denied_event_permission_keeps_other_resources_and_does_not_scan_events(mut scope: Scope) {
	scope.events_allowed = false;
	scope.events = vec![event(1, true)];
	scope.tasks = vec![task(2, true)];
	let result = snapshot_untracked(&mut scope, Uuid::from_u128(1))
		.await
		.unwrap();
	assert!(result.events.is_empty());
	assert_eq!(result.tasks.len(), 1);
	assert!(scope.event_pages.is_empty());
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn workspace_read_denial_precedes_any_snapshot_rows_or_membership_write(mut scope: Scope) {
	scope.require_allowed = false;
	assert!(matches!(
		snapshot(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["workspace", "require"]);
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn fitted_observation_tracks_only_the_selected_page_from_each_collection(mut scope: Scope) {
	scope.tasks = (1..=5).map(|id| task(id, true)).collect();
	scope.artifacts = (1..=5).map(|id| artifact(id, true)).collect();
	scope.events = (1..=5).rev().map(|seq| event(seq, true)).collect();
	scope.messages = (1..=5).rev().map(|id| message(id, true)).collect();
	let mut tried = vec![];
	let (limit, output) = observation_fitted(&mut scope, Uuid::from_u128(1), 1, 4, |limit, _| {
		tried.push(limit);
		Ok(limit == 2)
	})
	.await
	.unwrap()
	.unwrap();
	assert_eq!(tried, vec![4, 3, 2]);
	assert_eq!(limit, 2);
	assert_eq!(output["tasks"].as_array().unwrap().len(), 2);
	assert_eq!(scope.tracked.len(), 1);
	let tracked = &scope.tracked[0];
	assert_eq!(
		tracked.tasks.iter().map(|row| row.id).collect::<Vec<_>>(),
		vec![Uuid::from_u128(2), Uuid::from_u128(3)]
	);
	assert_eq!(
		tracked
			.artifacts
			.iter()
			.map(|row| row.id)
			.collect::<Vec<_>>(),
		vec![Uuid::from_u128(2), Uuid::from_u128(3)]
	);
	assert_eq!(
		tracked
			.events
			.iter()
			.map(|row| row.sequence)
			.collect::<Vec<_>>(),
		vec![4, 3]
	);
	assert_eq!(
		tracked
			.messages
			.iter()
			.map(|row| row.id)
			.collect::<Vec<_>>(),
		vec![Uuid::from_u128(4), Uuid::from_u128(3)]
	);
}
#[rstest]
#[tokio::test]
async fn an_unfittable_observation_does_not_publish_discarded_membership(mut scope: Scope) {
	scope.tasks = vec![task(1, true)];
	assert!(
		observation_fitted(&mut scope, Uuid::from_u128(1), 0, 2, |_, _| Ok(false))
			.await
			.unwrap()
			.is_none()
	);
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn fitting_failure_preserves_the_opaque_error_without_tracking(mut scope: Scope) {
	let Error::Port(error) = observation_fitted(&mut scope, Uuid::from_u128(1), 0, 2, |_, _| {
		Err(Error::Port(Box::new(std::io::Error::other("fit"))))
	})
	.await
	.unwrap_err() else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[case::ownership("workspace")]
#[case::read_policy("require")]
#[case::events_policy("decide")]
#[case::event_rows("event_rows")]
#[case::workspace_row("workspace_row")]
#[case::tasks("workspace_tasks")]
#[case::artifacts("workspace_artifacts")]
#[case::messages("message_rows")]
#[case::task_visibility("task_visible")]
#[case::artifact_visibility("artifact_visible")]
#[case::message_visibility("message_visible")]
#[case::event_visibility("event_visible")]
#[case::membership("track")]
#[tokio::test]
async fn snapshot_adapter_failures_cannot_publish_partial_disclosure(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.tasks = vec![task(1, true)];
	scope.artifacts = vec![artifact(1, true)];
	scope.messages = vec![message(1, true)];
	scope.events = vec![event(1, true)];
	scope.fail = Some(fail);
	let Error::Port(error) = snapshot(&mut scope, Uuid::from_u128(1)).await.unwrap_err() else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert!(scope.tracked.is_empty());
	assert_eq!(scope.calls.last(), Some(&fail));
}
