//! Visibility is applied before pages, snapshots and fitted observations are disclosed.
use crate::{
	Result,
	ports::authorization::{projection::WorkspaceProjection, visibility::RunVerdicts},
};
use aidash_domain::{
	Event, Message, Task, WorkspaceSnapshot,
	unconfirmed::{Kind, Tally},
	workspaces::TaskPage,
};
use serde_json::Value;
use uuid::Uuid;
pub const EVENT_SCAN_LIMIT: usize = 4096;
pub async fn task_page(
	scope: &mut dyn WorkspaceProjection,
	workspaces: &[Uuid],
	offset: u64,
) -> Result<TaskPage> {
	let mut cursor = offset;
	let mut tasks: Vec<Task> = vec![];
	let mut unconfirmed = Tally::new();
	loop {
		let rows = scope.task_rows(workspaces, cursor).await?;
		let exhausted = rows.len() < 500;
		for task in rows {
			cursor = cursor.saturating_add(1);
			scope.begin_row();
			let visible = scope.task_visible(&task).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle(silence, Kind::Task, task.workspace_id, visible) {
				tasks.push(task);
			}
			if tasks.len() == 500 {
				return Ok(TaskPage {
					tasks,
					next_offset: Some(cursor),
					unconfirmed: unconfirmed.into_entries(),
				});
			}
		}
		if exhausted {
			return Ok(TaskPage {
				tasks,
				next_offset: None,
				unconfirmed: unconfirmed.into_entries(),
			});
		}
	}
}
pub async fn latest_visible_events(
	scope: &mut dyn WorkspaceProjection,
	workspaces: &[Uuid],
	include_marketplace: bool,
) -> Result<Vec<Event>> {
	latest_visible_events_counted(scope, workspaces, include_marketplace)
		.await
		.map(|(events, _)| events)
}
/// The latest visible events and the events left out because a Node did not answer (#770).
pub async fn latest_visible_events_counted(
	scope: &mut dyn WorkspaceProjection,
	workspaces: &[Uuid],
	include_marketplace: bool,
) -> Result<(Vec<Event>, Tally)> {
	let mut cursor = i64::MAX;
	let mut result = vec![];
	let mut unconfirmed = Tally::new();
	let mut scanned = 0;
	// Every event of a Run would otherwise repeat that Run's full disclosure check (#292).
	let mut runs = RunVerdicts::default();
	while scanned < EVENT_SCAN_LIMIT {
		let page_size = (EVENT_SCAN_LIMIT - scanned).min(100);
		let rows = scope
			.event_rows(workspaces, include_marketplace, cursor, page_size)
			.await?;
		let exhausted = rows.len() < page_size;
		scanned += rows.len();
		for event in rows {
			cursor = event.sequence;
			scope.begin_row();
			let visible = scope.event_visible(&event, &mut runs).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle_in(silence, Kind::Event, event.workspace_id, visible) {
				result.push(event);
			}
			if result.len() == 100 {
				break;
			}
		}
		if exhausted || result.len() == 100 {
			break;
		}
	}
	result.reverse();
	Ok((result, unconfirmed))
}
pub async fn latest_visible_messages(
	scope: &mut dyn WorkspaceProjection,
	workspace: Uuid,
) -> Result<Vec<Message>> {
	latest_visible_messages_counted(scope, workspace)
		.await
		.map(|(messages, _)| messages)
}
/// The latest visible messages and the messages left out because a Node did not answer (#770).
pub async fn latest_visible_messages_counted(
	scope: &mut dyn WorkspaceProjection,
	workspace: Uuid,
) -> Result<(Vec<Message>, Tally)> {
	let mut offset = 0;
	let mut result = vec![];
	let mut unconfirmed = Tally::new();
	loop {
		let rows = scope.message_rows(workspace, offset).await?;
		let exhausted = rows.len() < 100;
		for message in rows {
			scope.begin_row();
			let visible = scope.message_visible(&message).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle(silence, Kind::Message, message.workspace_id, visible) {
				result.push(message);
			}
			if result.len() == 100 {
				break;
			}
		}
		if exhausted || result.len() == 100 {
			break;
		}
		offset += 100;
	}
	result.reverse();
	Ok((result, unconfirmed))
}
pub async fn snapshot_untracked(
	scope: &mut dyn WorkspaceProjection,
	id: Uuid,
) -> Result<WorkspaceSnapshot> {
	let workspace = scope.workspace(id).await?;
	scope.require(&workspace, "workspace.read").await?;
	let mut unconfirmed = Tally::new();
	let events = if scope.decide(&workspace, "workspace.events").await? {
		let (events, left_out) = latest_visible_events_counted(scope, &[id], false).await?;
		unconfirmed.merge(left_out);
		events
	} else {
		vec![]
	};
	let workspace_row = scope.workspace_row(id).await?;
	let workspace_tasks = scope.workspace_tasks(id).await?;
	let workspace_artifacts = scope.workspace_artifacts(id).await?;
	let (messages, left_out) = latest_visible_messages_counted(scope, id).await?;
	unconfirmed.merge(left_out);
	let mut snapshot = WorkspaceSnapshot {
		workspace: workspace_row,
		tasks: workspace_tasks,
		artifacts: workspace_artifacts,
		messages,
		events,
		unconfirmed: vec![],
	};
	let mut tasks = vec![];
	for task in snapshot.tasks {
		scope.begin_row();
		let visible = scope.task_visible(&task).await?;
		let silence = scope.take_row_silence();
		if unconfirmed.settle(silence, Kind::Task, task.workspace_id, visible) {
			tasks.push(task);
		}
	}
	snapshot.tasks = tasks;
	let mut artifacts = vec![];
	for artifact in snapshot.artifacts {
		scope.begin_row();
		let visible = scope.artifact_visible(&artifact).await?;
		let silence = scope.take_row_silence();
		if unconfirmed.settle(silence, Kind::Artifact, artifact.workspace_id, visible) {
			artifacts.push(artifact);
		}
	}
	snapshot.artifacts = artifacts;
	// Each message was judged by `latest_visible_messages` and is judged again
	// here. Messages dropped by the first pass are not in the list, so a message
	// dropped now, because a Node stopped answering in between, is counted once.
	let mut messages = vec![];
	for message in snapshot.messages {
		scope.begin_row();
		let visible = scope.message_visible(&message).await?;
		let silence = scope.take_row_silence();
		if unconfirmed.settle(silence, Kind::Message, message.workspace_id, visible) {
			messages.push(message);
		}
	}
	snapshot.messages = messages;
	snapshot.unconfirmed = unconfirmed.into_entries();
	Ok(snapshot)
}
pub async fn snapshot(scope: &mut dyn WorkspaceProjection, id: Uuid) -> Result<WorkspaceSnapshot> {
	let snapshot = snapshot_untracked(scope, id).await?;
	scope.track(&snapshot).await?;
	Ok(snapshot)
}
pub async fn observation_fitted<F>(
	scope: &mut dyn WorkspaceProjection,
	id: Uuid,
	offset: usize,
	limit: usize,
	fits: F,
) -> Result<Option<(usize, Value)>>
where
	F: FnMut(usize, &Value) -> Result<bool>,
{
	let snapshot = snapshot_untracked(scope, id).await?;
	let ids = snapshot
		.messages
		.iter()
		.map(|message| message.id)
		.collect::<Vec<_>>();
	let attachments = scope.message_attachments(id, &ids).await?;
	let enrich = |output: &mut Value| {
		if let Some(messages) = output["messages"].as_array_mut() {
			for message in messages {
				let id = message["id"]
					.as_str()
					.and_then(|value| value.parse::<Uuid>().ok());
				message["attachments"] = serde_json::json!(
					attachments
						.iter()
						.filter(|row| Some(row.message_id) == id)
						.map(|row| &row.attachment)
						.collect::<Vec<_>>()
				);
			}
		}
	};
	let mut fits = fits;
	let Some((fitted_limit, mut output)) = aidash_domain::context::observation::fit_projection(
		&snapshot,
		offset,
		limit,
		|n, candidate| {
			let mut candidate = candidate.clone();
			enrich(&mut candidate);
			fits(n, &candidate)
		},
	)?
	else {
		return Ok(None);
	};
	enrich(&mut output);
	let dependencies = WorkspaceSnapshot {
		workspace: snapshot.workspace.clone(),
		tasks: snapshot
			.tasks
			.iter()
			.skip(offset)
			.take(fitted_limit)
			.cloned()
			.collect(),
		artifacts: snapshot
			.artifacts
			.iter()
			.skip(offset)
			.take(fitted_limit)
			.cloned()
			.collect(),
		events: snapshot
			.events
			.iter()
			.rev()
			.skip(offset)
			.take(fitted_limit)
			.cloned()
			.collect(),
		messages: snapshot
			.messages
			.iter()
			.rev()
			.skip(offset)
			.take(fitted_limit)
			.cloned()
			.collect(),
		unconfirmed: vec![],
	};
	scope.track(&dependencies).await?;
	Ok(Some((fitted_limit, output)))
}
#[cfg(test)]
mod tests;

/// An unconstrained observation still records only its selected page before disclosure.
pub async fn observation(
	scope: &mut dyn WorkspaceProjection,
	id: Uuid,
	offset: usize,
	limit: usize,
) -> Result<Value> {
	observation_fitted(scope, id, offset, limit, |_, _| Ok(true))
		.await?
		.map(|(_, output)| output)
		.ok_or_else(|| crate::Error::Invalid("workspace observation could not be fitted".into()))
}
