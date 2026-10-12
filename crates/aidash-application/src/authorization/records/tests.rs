use super::*;
use crate::ports::authorization::records::ChildTaskRecord;
use aidash_domain::{Artifact, Event, Message, Task, TaskStatus, Workspace, policy::Resource};
use async_trait::async_trait;
use chrono::Utc;
use rstest::{fixture, rstest};
struct Scope {
	workspace: Workspace,
	task: Task,
	artifact: Artifact,
	message: Message,
	event: Event,
	calls: Vec<&'static str>,
	reads: Vec<(Uuid, Uuid)>,
	tracked: Vec<WorkspaceSnapshot>,
	required: bool,
	events_allowed: bool,
	found: bool,
	visible: bool,
	children: Vec<(Uuid, String, TaskStatus)>,
	cursors: Vec<Option<Uuid>>,
	task_reads: Vec<Vec<Uuid>>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	let now = Utc::now();
	let workspace = Uuid::from_u128(1);
	Scope {
		workspace: Workspace {
			id: workspace,
			title: "workspace".into(),
			goal: "goal".into(),
			state: json!({"saved":true}),
			revision: 9,
			created_at: now,
		},
		task: Task {
			id: Uuid::from_u128(11),
			workspace_id: workspace,
			title: "task".into(),
			description: "description".into(),
			status: TaskStatus::Open,
			requirements: json!({}),
			owner: None,
			created_by: "saved-author".into(),
			dependencies: vec![],
			parent_id: None,
			revision: 3,
			created_at: now,
		},
		artifact: Artifact {
			id: Uuid::from_u128(12),
			workspace_id: workspace,
			task_id: Uuid::from_u128(11),
			kind: "text".into(),
			name: "artifact".into(),
			content: json!({"text":"日本語\u{0000}","nested":[1,2]}),
			created_by: "saved-author".into(),
			idempotency_key: "artifact".into(),
			created_at: now,
		},
		message: Message {
			id: Uuid::from_u128(13),
			workspace_id: workspace,
			sender: "saved-sender".into(),
			content: "日本語\u{0000}".into(),
			idempotency_key: None,
			created_at: now,
		},
		event: Event {
			sequence: 15,
			id: Uuid::from_u128(14),
			node_id: "aidash://local".into(),
			workspace_id: Some(workspace),
			kind: "message.created".into(),
			data: json!({"id":Uuid::from_u128(13)}),
			created_at: now,
		},
		calls: vec![],
		reads: vec![],
		tracked: vec![],
		required: true,
		events_allowed: true,
		found: true,
		visible: true,
		children: vec![],
		cursors: vec![],
		task_reads: vec![],
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
	fn resource(&self) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: self.workspace.id.to_string(),
			attributes: json!({"owner":"saved-owner"}),
		}
	}
	async fn owned_resource(&mut self, id: Uuid) -> Result<Resource> {
		assert_eq!(id, self.workspace.id);
		self.touch("workspace")?;
		Ok(self.resource())
	}
	async fn required(&mut self, resource: &Resource, action: &str) -> Result<()> {
		assert_eq!(resource.attributes["owner"], "saved-owner");
		assert_eq!(action, "workspace.read");
		self.touch("require")?;
		if self.required {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
}
#[async_trait]
impl WorkspaceRecordScope for Scope {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.owned_resource(id).await
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.required(resource, action).await
	}
	async fn decide(&mut self, _: &Resource, action: &str) -> Result<bool> {
		assert_eq!(action, "workspace.events");
		self.touch("decide")?;
		Ok(self.events_allowed)
	}
	async fn workspace_row(&mut self, id: Uuid) -> Result<Workspace> {
		assert_eq!(id, self.workspace.id);
		self.touch("workspace_row")?;
		Ok(self.workspace.clone())
	}
	async fn task_record(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Task>> {
		self.touch("task")?;
		self.reads.push((workspace, id));
		Ok(self.found.then(|| self.task.clone()))
	}
	async fn artifact_record(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Artifact>> {
		self.touch("artifact")?;
		self.reads.push((workspace, id));
		Ok(self.found.then(|| self.artifact.clone()))
	}
	async fn message_record(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Message>> {
		self.touch("message")?;
		self.reads.push((workspace, id));
		Ok(self.found.then(|| self.message.clone()))
	}
	async fn event_record(&mut self, workspace: Uuid, id: Uuid) -> Result<Option<Event>> {
		self.touch("event")?;
		self.reads.push((workspace, id));
		Ok(self.found.then(|| self.event.clone()))
	}
	async fn task_visible(&mut self, row: &Task) -> Result<bool> {
		assert_eq!(row.created_by, "saved-author");
		self.touch("task_visible")?;
		Ok(self.visible)
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		assert_eq!(row.created_by, "saved-author");
		self.touch("artifact_visible")?;
		Ok(self.visible)
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		assert_eq!(row.sender, "saved-sender");
		self.touch("message_visible")?;
		Ok(self.visible)
	}
	async fn event_visible(&mut self, row: &Event) -> Result<bool> {
		assert_eq!(row.sequence, 15);
		self.touch("event_visible")?;
		Ok(self.visible)
	}
	async fn track(&mut self, snapshot: &WorkspaceSnapshot) -> Result<()> {
		self.touch("track")?;
		self.tracked.push(snapshot.clone());
		Ok(())
	}
}
#[async_trait]
impl ChildSummaryScope for Scope {
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.owned_resource(id).await
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.required(resource, action).await
	}
	async fn children(
		&mut self,
		workspace_id: Uuid,
		parent_id: Uuid,
		after: Option<Uuid>,
	) -> Result<Vec<ChildTaskRecord>> {
		assert_eq!(workspace_id, self.workspace.id);
		assert_eq!(parent_id, Uuid::from_u128(99));
		self.touch("children")?;
		self.cursors.push(after);
		Ok(self
			.children
			.iter()
			.filter(|row| after.is_none_or(|after| row.0 > after))
			.take(100)
			.map(|row| ChildTaskRecord {
				id: row.0,
				created_by: row.1.clone(),
				status: row.2,
			})
			.collect())
	}
	async fn visible(
		&mut self,
		resource: &Resource,
		workspace: Uuid,
		_: Uuid,
		created_by: &str,
	) -> Result<bool> {
		assert_eq!(resource.attributes["owner"], "saved-owner");
		assert_eq!(workspace, self.workspace.id);
		self.touch("summary_visible")?;
		Ok(created_by == "visible")
	}
	async fn track_tasks(&mut self, workspace: Uuid, tasks: &[Uuid]) -> Result<()> {
		assert_eq!(workspace, self.workspace.id);
		self.touch("task_track")?;
		self.task_reads.push(tasks.to_vec());
		Ok(())
	}
}
#[rstest]
#[case::workspace("workspace", 1)]
#[case::task("task", 11)]
#[case::artifact("artifact", 12)]
#[case::message("message", 13)]
#[case::event("event", 14)]
#[tokio::test]
async fn a_record_returns_the_original_serialized_value_after_exact_membership_tracking(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] id: u128,
) {
	let expected = match kind {
		"workspace" => json!(scope.workspace),
		"task" => json!(scope.task),
		"artifact" => json!(scope.artifact),
		"message" => json!(scope.message),
		_ => json!(scope.event),
	};
	let result = record(&mut scope, Uuid::from_u128(1), kind, Uuid::from_u128(id))
		.await
		.unwrap();
	assert_eq!(result, expected);
	assert_eq!(scope.tracked.len(), 1);
	let snapshot = &scope.tracked[0];
	assert_eq!(snapshot.workspace.id, Uuid::from_u128(1));
	assert_eq!(snapshot.tasks.len(), usize::from(kind == "task"));
	assert_eq!(snapshot.artifacts.len(), usize::from(kind == "artifact"));
	assert_eq!(snapshot.messages.len(), usize::from(kind == "message"));
	assert_eq!(snapshot.events.len(), usize::from(kind == "event"));
	assert_eq!(scope.calls.last(), Some(&"track"));
	if kind != "workspace" {
		assert_eq!(scope.reads, vec![(Uuid::from_u128(1), Uuid::from_u128(id))]);
	}
}
#[rstest]
#[case::task("task")]
#[case::artifact("artifact")]
#[case::message("message")]
#[case::event("event")]
#[tokio::test]
async fn missing_records_are_denied_without_a_membership_commit(
	mut scope: Scope,
	#[case] kind: &str,
) {
	scope.found = false;
	assert!(matches!(
		record(&mut scope, Uuid::from_u128(1), kind, Uuid::from_u128(20)).await,
		Err(Error::Forbidden)
	));
	assert!(scope.tracked.is_empty());
	assert_eq!(scope.calls.last(), Some(&kind));
}
fn record_id(kind: &str) -> Uuid {
	Uuid::from_u128(match kind {
		"task" => 11,
		"artifact" => 12,
		"message" => 13,
		"event" => 14,
		_ => 1,
	})
}
#[rstest]
#[case::task("task", "task_visible")]
#[case::artifact("artifact", "artifact_visible")]
#[case::message("message", "message_visible")]
#[case::event("event", "event_visible")]
#[tokio::test]
async fn current_record_visibility_precedes_disclosure_and_tracking(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] last: &'static str,
) {
	scope.visible = false;
	assert!(matches!(
		record(&mut scope, Uuid::from_u128(1), kind, record_id(kind)).await,
		Err(Error::Forbidden)
	));
	assert!(scope.tracked.is_empty());
	assert_eq!(scope.calls.last(), Some(&last));
}
#[rstest]
#[tokio::test]
async fn event_read_permission_is_checked_before_the_event_row(mut scope: Scope) {
	scope.events_allowed = false;
	assert!(matches!(
		record(&mut scope, Uuid::from_u128(1), "event", Uuid::from_u128(14)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec!["workspace", "require", "workspace_row", "decide"]
	);
	assert!(scope.reads.is_empty());
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn full_workspace_read_requires_the_selected_workspace_identity(mut scope: Scope) {
	assert!(matches!(
		record(
			&mut scope,
			Uuid::from_u128(1),
			"workspace",
			Uuid::from_u128(2)
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["workspace", "require", "workspace_row"]);
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn unknown_record_kind_keeps_validation_after_workspace_authority(mut scope: Scope) {
	let error = record(
		&mut scope,
		Uuid::from_u128(1),
		"unknown",
		Uuid::from_u128(2),
	)
	.await
	.unwrap_err();
	assert!(
		matches!(error,Error::Invalid(ref message) if message=="unknown workspace record kind")
	);
	assert_eq!(scope.calls, vec!["workspace", "require", "workspace_row"]);
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[tokio::test]
async fn workspace_read_denial_precedes_record_lookup(mut scope: Scope) {
	scope.required = false;
	assert!(matches!(
		record(&mut scope, Uuid::from_u128(1), "task", Uuid::from_u128(11)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["workspace", "require"]);
	assert!(scope.reads.is_empty());
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[case::workspace("workspace", "workspace")]
#[case::read_policy("workspace", "require")]
#[case::workspace_row("workspace", "workspace_row")]
#[case::task("task", "task")]
#[case::task_policy("task", "task_visible")]
#[case::artifact("artifact", "artifact")]
#[case::artifact_policy("artifact", "artifact_visible")]
#[case::message("message", "message")]
#[case::message_policy("message", "message_visible")]
#[case::event_permission("event", "decide")]
#[case::event("event", "event")]
#[case::event_policy("event", "event_visible")]
#[case::journal("workspace", "track")]
#[tokio::test]
async fn record_failures_keep_their_opaque_identity_without_returning_partial_records(
	mut scope: Scope,
	#[case] kind: &str,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = record(&mut scope, Uuid::from_u128(1), kind, record_id(kind))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last(), Some(&fail));
	assert!(scope.tracked.is_empty());
}
#[rstest]
#[case::open(TaskStatus::Open, true, false)]
#[case::claimed(TaskStatus::Claimed, true, false)]
#[case::running(TaskStatus::Running, true, false)]
#[case::completed(TaskStatus::Completed, false, false)]
#[case::failed(TaskStatus::Failed, true, true)]
#[case::blocked(TaskStatus::Blocked, true, true)]
#[case::cancelled(TaskStatus::Cancelled, true, true)]
#[case::abandoned(TaskStatus::Abandoned, false, false)]
#[tokio::test]
async fn visible_child_summary_preserves_each_domain_status(
	mut scope: Scope,
	#[case] status: TaskStatus,
	#[case] pending: bool,
	#[case] failed: bool,
) {
	scope.children = vec![(Uuid::from_u128(2), "visible".into(), status)];
	let summary = children(&mut scope, Uuid::from_u128(1), Uuid::from_u128(99))
		.await
		.unwrap();
	assert_eq!(
		summary,
		ChildTaskSummary {
			has_pending: pending,
			has_failed: failed
		}
	);
	assert_eq!(scope.task_reads, vec![vec![Uuid::from_u128(2)]]);
}
#[rstest]
#[tokio::test]
async fn hidden_child_status_does_not_contribute_to_summary_or_membership(mut scope: Scope) {
	scope.children = vec![
		(Uuid::from_u128(2), "hidden".into(), TaskStatus::Failed),
		(Uuid::from_u128(3), "visible".into(), TaskStatus::Completed),
	];
	let summary = children(&mut scope, Uuid::from_u128(1), Uuid::from_u128(99))
		.await
		.unwrap();
	assert_eq!(
		summary,
		ChildTaskSummary {
			has_pending: false,
			has_failed: false
		}
	);
	assert_eq!(scope.task_reads, vec![vec![Uuid::from_u128(3)]]);
}
#[rstest]
#[tokio::test]
async fn child_summary_advances_over_hidden_pages_and_tracks_each_visible_page(mut scope: Scope) {
	scope.children = (1..=205)
		.map(|id| {
			(
				Uuid::from_u128(id),
				if id > 150 { "visible" } else { "hidden" }.into(),
				TaskStatus::Failed,
			)
		})
		.collect();
	let summary = children(&mut scope, Uuid::from_u128(1), Uuid::from_u128(99))
		.await
		.unwrap();
	assert!(summary.has_pending);
	assert!(summary.has_failed);
	assert_eq!(
		scope.cursors,
		vec![None, Some(Uuid::from_u128(100)), Some(Uuid::from_u128(200))]
	);
	assert_eq!(
		scope.task_reads,
		vec![
			vec![],
			(151..=200).map(Uuid::from_u128).collect(),
			(201..=205).map(Uuid::from_u128).collect()
		]
	);
}
#[rstest]
#[case::empty(0,vec![None])]
#[case::full(100,vec![None,Some(Uuid::from_u128(100))])]
#[tokio::test]
async fn child_summary_keeps_empty_and_exact_full_page_exhaustion(
	mut scope: Scope,
	#[case] rows: u128,
	#[case] cursors: Vec<Option<Uuid>>,
) {
	scope.children = (1..=rows)
		.map(|id| (Uuid::from_u128(id), "visible".into(), TaskStatus::Completed))
		.collect();
	let summary = children(&mut scope, Uuid::from_u128(1), Uuid::from_u128(99))
		.await
		.unwrap();
	assert!(!summary.has_pending);
	assert!(!summary.has_failed);
	assert_eq!(scope.cursors, cursors);
	assert!(scope.task_reads.last().unwrap().is_empty());
}
#[rstest]
#[case::ownership("workspace")]
#[case::permission("require")]
#[case::rows("children")]
#[case::policy("summary_visible")]
#[case::journal("task_track")]
#[tokio::test]
async fn child_summary_errors_preserve_their_opaque_identity(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.children = vec![(Uuid::from_u128(2), "visible".into(), TaskStatus::Failed)];
	scope.fail = Some(fail);
	let Error::Port(error) = children(&mut scope, Uuid::from_u128(1), Uuid::from_u128(99))
		.await
		.unwrap_err()
	else {
		panic!("expected opaque error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(scope.calls.last(), Some(&fail));
	assert!(scope.task_reads.is_empty());
}

#[rstest]
#[case("readable")]
#[case("denied")]
#[case("missing")]
#[case("hidden")]
#[tokio::test]
async fn summary_dependencies_recheck_messages_without_tracking(
	mut scope: Scope,
	#[case] state: &str,
) {
	match state {
		"denied" => scope.required = false,
		"missing" => scope.found = false,
		"hidden" => scope.visible = false,
		_ => {}
	}
	let workspace = scope.workspace.id;
	let readable = messages_readable(&mut scope, workspace, [Uuid::from_u128(13)])
		.await
		.unwrap();
	assert_eq!(readable, state == "readable");
	assert!(scope.tracked.is_empty(), "a recheck records no new read");
	assert!(!scope.calls.contains(&"track"));
}
