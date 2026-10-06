//! Dashboard state applies current disclosure policy before bounded projections.
use crate::{
	Result,
	ports::authorization::state::{WorkspaceListingScope, WorkspaceStateScope},
};
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunInspection, Task, Workspace, registry::Entry,
	workspaces::TaskPage,
};
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
}
pub async fn task_page(scope: &mut dyn WorkspaceListingScope, offset: u64) -> Result<TaskPage> {
	let workspaces = scope.visible("workspace.read").await?;
	scope.task_page(&workspaces, offset).await
}
pub async fn state(scope: &mut dyn WorkspaceStateScope) -> Result<WorkspaceState> {
	let visible = scope.visible("workspace.read").await?;
	let mut event_workspaces = vec![];
	for id in &visible {
		if scope.allowed(*id, "workspace.events").await? {
			event_workspaces.push(*id);
		}
	}
	let mut state = WorkspaceState {
		registry: scope.registry().await?,

		workspaces: scope.workspace_rows(&visible).await?,
		tasks: scope.task_page(&visible, 0).await?.tasks,
		artifacts: vec![],
		runs: vec![],
		human_requests: vec![],
		conversations: vec![],
		events: scope.latest_visible_events(&event_workspaces, true).await?,
	};
	let mut offset = 0_u64;
	loop {
		let batch: Vec<Artifact> = scope.artifact_rows(&visible, offset).await?;
		let exhausted = batch.len() < 500;
		for artifact in batch {
			if scope.artifact_visible(&artifact).await? {
				state.artifacts.push(artifact);
			}
			if state.artifacts.len() == 500 {
				break;
			}
		}
		if exhausted || state.artifacts.len() == 500 {
			break;
		}
		offset += 500;
	}
	let mut offset = 0_i64;
	loop {
		let batch: Vec<aidash_domain::run_state::RawRun> = scope.raw_runs(&visible, offset).await?;
		let exhausted = batch.len() < 500;
		for run in batch {
			if scope.run_visible(&run.metadata).await? {
				state.runs.push(run.inspect());
			}
			if state.runs.len() == 500 {
				break;
			}
		}
		if exhausted || state.runs.len() == 500 {
			break;
		}
		offset += 500;
	}
	let run_ids: Vec<Uuid> = state.runs.iter().map(|run| run.id).collect();
	let mut offset = 0_i64;
	loop {
		let batch: Vec<HumanRequest> = scope.human_rows(&run_ids, offset).await?;
		let exhausted = batch.len() < 500;
		for request in batch {
			let resource = scope.human_resource(&request).await?;
			if scope.decide(&resource, "human.read").await? {
				state.human_requests.push(request);
			}
			if state.human_requests.len() == 500 {
				break;
			}
		}
		if exhausted || state.human_requests.len() == 500 {
			break;
		}
		offset += 500;
	}
	let mut offset = 0_i64;
	loop {
		let batch: Vec<Conversation> = scope.conversation_rows(&visible, offset).await?;
		let exhausted = batch.len() < 500;
		for conversation in batch {
			let resource = scope.conversation_resource(&conversation).await?;
			if scope.decide(&resource, "conversation.read").await? {
				state.conversations.push(conversation);
			}
			if state.conversations.len() == 500 {
				break;
			}
		}
		if exhausted || state.conversations.len() == 500 {
			break;
		}
		offset += 500;
	}

	Ok(state)
}
#[cfg(test)]
mod tests;
