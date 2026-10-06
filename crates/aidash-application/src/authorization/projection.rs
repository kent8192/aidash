//! Visibility is applied before pages, snapshots and fitted observations are disclosed.
use crate::{Result, ports::authorization::projection::WorkspaceProjection};
use aidash_domain::{Event, Message, Task, WorkspaceSnapshot, workspaces::TaskPage};
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
	loop {
		let rows = scope.task_rows(workspaces, cursor).await?;
		let exhausted = rows.len() < 500;
		for task in rows {
			cursor = cursor.saturating_add(1);
			if scope.task_visible(&task).await? {
				tasks.push(task);
			}
			if tasks.len() == 500 {
				return Ok(TaskPage {
					tasks,
					next_offset: Some(cursor),
				});
			}
		}
		if exhausted {
			return Ok(TaskPage {
				tasks,
				next_offset: None,
			});
		}
	}
}
pub async fn latest_visible_events(
	scope: &mut dyn WorkspaceProjection,
	workspaces: &[Uuid],
	include_marketplace: bool,
) -> Result<Vec<Event>> {
	let mut cursor = i64::MAX;
	let mut result = vec![];
	let mut scanned = 0;
	while scanned < EVENT_SCAN_LIMIT {
		let page_size = (EVENT_SCAN_LIMIT - scanned).min(100);
		let rows = scope
			.event_rows(workspaces, include_marketplace, cursor, page_size)
			.await?;
		let exhausted = rows.len() < page_size;
		scanned += rows.len();
		for event in rows {
			cursor = event.sequence;
			if scope.event_visible(&event).await? {
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
	Ok(result)
}
pub async fn latest_visible_messages(
	scope: &mut dyn WorkspaceProjection,
	workspace: Uuid,
) -> Result<Vec<Message>> {
	let mut offset = 0;
	let mut result = vec![];
	loop {
		let rows = scope.message_rows(workspace, offset).await?;
		let exhausted = rows.len() < 100;
		for message in rows {
			if scope.message_visible(&message).await? {
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
	Ok(result)
}
pub async fn snapshot_untracked(
	scope: &mut dyn WorkspaceProjection,
	id: Uuid,
) -> Result<WorkspaceSnapshot> {
	let workspace = scope.workspace(id).await?;
	scope.require(&workspace, "workspace.read").await?;
	let events = if scope.decide(&workspace, "workspace.events").await? {
		latest_visible_events(scope, &[id], false).await?
	} else {
		vec![]
	};
	let mut snapshot = WorkspaceSnapshot {
		workspace: scope.workspace_row(id).await?,
		tasks: scope.workspace_tasks(id).await?,
		artifacts: scope.workspace_artifacts(id).await?,
		messages: latest_visible_messages(scope, id).await?,
		events,
	};
	let mut tasks = vec![];
	for task in snapshot.tasks {
		if scope.task_visible(&task).await? {
			tasks.push(task);
		}
	}
	snapshot.tasks = tasks;
	let mut artifacts = vec![];
	for artifact in snapshot.artifacts {
		if scope.artifact_visible(&artifact).await? {
			artifacts.push(artifact);
		}
	}
	snapshot.artifacts = artifacts;
	let mut messages = vec![];
	for message in snapshot.messages {
		if scope.message_visible(&message).await? {
			messages.push(message);
		}
	}
	snapshot.messages = messages;
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
	let Some((fitted_limit, output)) =
		aidash_domain::context::observation::fit_projection(&snapshot, offset, limit, fits)?
	else {
		return Ok(None);
	};
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
