//! Full records and bounded child summaries apply current policy before disclosure.
use crate::{
	Error, Result,
	ports::authorization::records::{ChildSummaryScope, WorkspaceRecordScope},
};
use aidash_domain::{ChildTaskSummary, WorkspaceSnapshot};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn record(
	scope: &mut dyn WorkspaceRecordScope,
	workspace_id: Uuid,
	kind: &str,
	id: Uuid,
) -> Result<Value> {
	let resource = scope.workspace(workspace_id).await?;
	scope.require(&resource, "workspace.read").await?;
	let mut snapshot = WorkspaceSnapshot {
		workspace: scope.workspace_row(workspace_id).await?,
		tasks: vec![],
		artifacts: vec![],
		events: vec![],
		messages: vec![],
	};
	let value = match kind {
		"workspace" if id == workspace_id => json!(snapshot.workspace.clone()),
		"workspace" => return Err(Error::Forbidden),
		"task" => {
			let task = scope
				.task_record(workspace_id, id)
				.await?
				.ok_or(Error::Forbidden)?;
			if !scope.task_visible(&task).await? {
				return Err(Error::Forbidden);
			}
			snapshot.tasks.push(task.clone());
			json!(task)
		}
		"artifact" => {
			let artifact = scope
				.artifact_record(workspace_id, id)
				.await?
				.ok_or(Error::Forbidden)?;
			if !scope.artifact_visible(&artifact).await? {
				return Err(Error::Forbidden);
			}
			snapshot.artifacts.push(artifact.clone());
			json!(artifact)
		}
		"message" => {
			let message = scope
				.message_record(workspace_id, id)
				.await?
				.ok_or(Error::Forbidden)?;
			if !scope.message_visible(&message).await? {
				return Err(Error::Forbidden);
			}
			snapshot.messages.push(message.clone());
			json!(message)
		}
		"event" => {
			if !scope.decide(&resource, "workspace.events").await? {
				return Err(Error::Forbidden);
			}
			let event = scope
				.event_record(workspace_id, id)
				.await?
				.ok_or(Error::Forbidden)?;
			if !scope.event_visible(&event).await? {
				return Err(Error::Forbidden);
			}
			snapshot.events.push(event.clone());
			json!(event)
		}
		_ => return Err(Error::Invalid("unknown workspace record kind".into())),
	};
	scope.track(&snapshot).await?;
	Ok(value)
}
/// Recheck that every message is still readable under current authority,
/// applying the same policy as `record` without disclosing or tracking a read.
pub async fn messages_readable(
	scope: &mut dyn WorkspaceRecordScope,
	workspace_id: Uuid,
	ids: impl IntoIterator<Item = Uuid> + Send,
) -> Result<bool> {
	let resource = scope.workspace(workspace_id).await?;
	match scope.require(&resource, "workspace.read").await {
		Ok(()) => {}
		Err(Error::Forbidden | Error::Unauthorized) => return Ok(false),
		Err(error) => return Err(error),
	}
	for id in ids {
		let Some(message) = scope.message_record(workspace_id, id).await? else {
			return Ok(false);
		};
		if !scope.message_visible(&message).await? {
			return Ok(false);
		}
	}
	Ok(true)
}
pub async fn children(
	scope: &mut dyn ChildSummaryScope,
	workspace_id: Uuid,
	parent_id: Uuid,
) -> Result<ChildTaskSummary> {
	let resource = scope.workspace(workspace_id).await?;
	scope.require(&resource, "workspace.read").await?;
	let mut summary = ChildTaskSummary {
		has_pending: false,
		has_failed: false,
	};
	let mut after = None;
	loop {
		let rows = scope.children(workspace_id, parent_id, after).await?;
		let exhausted = rows.len() < 100;
		let mut visible_ids = Vec::with_capacity(rows.len());
		for row in rows {
			after = Some(row.id);
			if !scope
				.visible(&resource, workspace_id, row.id, &row.created_by)
				.await?
			{
				continue;
			}
			visible_ids.push(row.id);
			summary.include_status(row.status);
		}
		scope.track_tasks(workspace_id, &visible_ids).await?;
		if exhausted {
			return Ok(summary);
		}
	}
}
#[cfg(test)]
mod tests;
