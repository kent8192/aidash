//! Dashboard state applies current disclosure policy before bounded projections.
use crate::{
	Result,
	ports::authorization::state::{WorkspaceListingScope, WorkspaceStateScope},
};
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunInspection, RunState, Task, WaitingState,
	Workspace, registry::Entry, workspaces::TaskPage,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;
pub struct WorkspaceState {
	pub registry: Vec<Entry>,
	pub workspaces: Vec<Workspace>,
	pub tasks: Vec<Task>,
	pub artifacts: Vec<Artifact>,
	pub runs: Vec<RunInspection>,
	pub human_requests: Vec<HumanRequest>,
	pub conversations: Vec<Conversation>,
	pub events: Vec<Event>,
	pub next_cursors: NextCursors,
}
pub async fn task_page(scope: &mut dyn WorkspaceListingScope, offset: u64) -> Result<TaskPage> {
	let workspaces = scope.visible("workspace.read").await?;
	scope.task_page(&workspaces, offset).await
}
#[derive(Default, Clone, Debug, serde::Serialize, schemars::JsonSchema)]
pub struct NextCursors {
	pub workspaces: Option<String>,
	pub tasks: Option<String>,
	pub runs: Option<String>,
	pub artifacts: Option<String>,
	pub human_requests: Option<String>,
	pub conversations: Option<String>,
}

fn cursor<T>(rows: &[T], id: impl Fn(&T) -> Uuid) -> Option<String> {
	(rows.len() == super::bounded_reads::PAGE_SIZE)
		.then(|| id(rows.last().expect("full page is not empty")).to_string())
}

async fn workspace_ok(
	scope: &mut dyn WorkspaceStateScope,
	cache: &mut HashMap<Uuid, bool>,
	id: Uuid,
) -> Result<bool> {
	if let Some(allowed) = cache.get(&id) {
		return Ok(*allowed);
	}
	let allowed = scope.allowed(id, "workspace.read").await?;
	cache.insert(id, allowed);
	Ok(allowed)
}

pub async fn state(
	scope: &mut dyn WorkspaceStateScope,
	node: Option<&str>,
) -> Result<WorkspaceState> {
	let mut cache = HashMap::new();
	let mut workspace_ids = Vec::new();
	let workspace_candidates = scope.workspace_candidates(node, None).await?;
	let mut cursors = NextCursors {
		workspaces: cursor(&workspace_candidates, |id| *id),
		..Default::default()
	};
	for id in workspace_candidates {
		if workspace_ok(scope, &mut cache, id).await? {
			workspace_ids.push(id);
		}
	}
	let mut tasks = Vec::new();
	let candidates = scope.task_candidates(node).await?;
	cursors.tasks = cursor(&candidates, |row| row.id);
	for row in candidates {
		if workspace_ok(scope, &mut cache, row.workspace_id).await?
			&& scope.task_visible(&row).await?
		{
			workspace_ids.push(row.workspace_id);
			tasks.push(row);
		}
	}
	let mut artifacts = Vec::new();
	let candidates = scope.artifact_candidates(node).await?;
	cursors.artifacts = cursor(&candidates, |row| row.id);
	for row in candidates {
		if workspace_ok(scope, &mut cache, row.workspace_id).await?
			&& scope.artifact_visible(&row).await?
		{
			workspace_ids.push(row.workspace_id);
			artifacts.push(row);
		}
	}
	let mut runs = Vec::new();
	let candidates = scope.run_candidates(node).await?;
	cursors.runs = cursor(&candidates, |row| row.metadata.id);
	for row in candidates {
		if workspace_ok(scope, &mut cache, row.metadata.workspace_id).await?
			&& scope.run_visible(&row.metadata).await?
		{
			workspace_ids.push(row.metadata.workspace_id);
			runs.push(row.inspect());
		}
	}
	let mut human_requests = Vec::new();
	let mut visible_runs = HashMap::new();
	let candidates = scope.pending_human_candidates(node).await?;
	cursors.human_requests = cursor(&candidates, |row| row.id);
	for mut row in candidates {
		if !workspace_ok(scope, &mut cache, row.workspace_id).await? {
			continue;
		}
		if let std::collections::hash_map::Entry::Vacant(entry) = visible_runs.entry(row.run_id) {
			entry.insert(scope.visible_run(row.run_id).await?);
		}
		let Some(run) = visible_runs.get(&row.run_id).and_then(Option::as_ref) else {
			continue;
		};
		let resource = scope.human_resource(&row).await?;
		if scope.decide(&resource, "human.read").await? {
			row.answer_by = answer_deadlines(std::slice::from_ref(run))
				.get(&row.id)
				.copied();
			workspace_ids.push(row.workspace_id);
			human_requests.push(row);
		}
	}
	let mut conversations = Vec::new();
	let candidates = scope.conversation_candidates(node).await?;
	cursors.conversations = cursor(&candidates, |row| row.id);
	for row in candidates {
		if !workspace_ok(scope, &mut cache, row.workspace_id).await? {
			continue;
		}
		let resource = scope.conversation_resource(&row).await?;
		if scope.decide(&resource, "conversation.read").await? {
			workspace_ids.push(row.workspace_id);
			conversations.push(row);
		}
	}
	workspace_ids.sort_unstable();
	workspace_ids.dedup();
	Ok(WorkspaceState {
		registry: scope.registry().await?,
		workspaces: scope.workspace_rows(&workspace_ids).await?,
		tasks,
		artifacts,
		runs,
		human_requests,
		conversations,
		events: scope.latest_visible_events(node).await?,
		next_cursors: cursors,
	})
}
/// When each question that Runs wait on continues without an answer, by request.
pub fn answer_deadlines(runs: &[RunInspection]) -> HashMap<Uuid, DateTime<Utc>> {
	runs.iter()
		.filter_map(|run| match &run.state {
			Some(RunState::Waiting(wait)) => match wait.as_ref() {
				WaitingState::Human {
					request_id,
					answer_by: Some(by),
					..
				} => Some((*request_id, *by)),
				_ => None,
			},
			_ => None,
		})
		.collect()
}
#[cfg(test)]
mod tests;
