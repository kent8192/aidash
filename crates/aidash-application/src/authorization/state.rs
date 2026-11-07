//! Dashboard state applies current disclosure policy before bounded projections.
use crate::{
	Result,
	ports::authorization::state::{WorkspaceListingScope, WorkspaceStateScope},
};
use aidash_domain::{
	Artifact, Conversation, Event, HumanRequest, RunInspection, RunState, Task, WaitingState,
	Workspace,
	registry::Entry,
	unconfirmed::{Kind, Tally, Unconfirmed},
	workspaces::TaskPage,
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
	/// Rows this read scanned and could not confirm because a Node did not answer (#770).
	pub unconfirmed: Vec<Unconfirmed>,
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
	let mut unconfirmed = Tally::new();
	let registry = scope.registry().await?;
	let workspaces = scope.workspace_rows(&visible).await?;
	let tasks = scope.task_page(&visible, 0).await?;
	unconfirmed.merge(Tally::from_entries(tasks.unconfirmed));
	let (events, left_out) = scope.latest_visible_events(&event_workspaces, true).await?;
	unconfirmed.merge(left_out);
	let mut state = WorkspaceState {
		registry,
		workspaces,
		tasks: tasks.tasks,
		artifacts: vec![],
		runs: vec![],
		human_requests: vec![],
		conversations: vec![],
		events,
		unconfirmed: vec![],
	};
	let mut offset = 0_u64;
	loop {
		let batch: Vec<Artifact> = scope.artifact_rows(&visible, offset).await?;
		let exhausted = batch.len() < 500;
		for artifact in batch {
			scope.begin_row();
			let allowed = scope.artifact_visible(&artifact).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle(silence, Kind::Artifact, artifact.workspace_id, allowed) {
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
	// Runs left out for a silent Node, with the Node and Workspace, so their
	// Human Requests are counted too (#770).
	let mut silent_runs: HashMap<Uuid, (String, Uuid)> = HashMap::new();
	let mut offset = 0_i64;
	loop {
		let batch: Vec<aidash_domain::run_state::RawRun> = scope.raw_runs(&visible, offset).await?;
		let exhausted = batch.len() < 500;
		for run in batch {
			scope.begin_row();
			let allowed = scope.run_visible(&run.metadata).await?;
			let silence = scope.take_row_silence();
			if let Some(node) = silence.as_ref().filter(|_| !allowed) {
				silent_runs.insert(run.metadata.id, (node.clone(), run.metadata.workspace_id));
			}
			if unconfirmed.settle(silence, Kind::Run, run.metadata.workspace_id, allowed) {
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
	// A Human Request is read only for the Runs that survived, so a request of
	// a Run left out is counted here, after the reader's own `human.read`.
	if !silent_runs.is_empty() {
		let silent_ids: Vec<Uuid> = silent_runs.keys().copied().collect();
		let mut offset = 0_i64;
		let mut scanned = 0;
		loop {
			let batch: Vec<HumanRequest> = scope.human_rows(&silent_ids, offset).await?;
			let exhausted = batch.len() < 500;
			scanned += batch.len();
			for request in batch {
				let resource = scope.human_resource(&request).await?;
				if scope.decide(&resource, "human.read").await?
					&& let Some((node, _)) = silent_runs.get(&request.run_id)
				{
					unconfirmed.record(node, Kind::HumanRequest, request.workspace_id);
				}
			}
			if exhausted || scanned >= 2000 {
				break;
			}
			offset += 500;
		}
	}
	let continues_by = answer_deadlines(&state.runs);
	for request in &mut state.human_requests {
		request.answer_by = continues_by
			.get(&request.id)
			.copied()
			.filter(|_| request.response.is_none());
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

	state.unconfirmed = unconfirmed.into_entries();
	Ok(state)
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
