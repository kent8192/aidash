//! Full records and bounded child summaries apply current policy before disclosure.
use crate::{
	Error, Result,
	ports::authorization::records::{ChildSummaryScope, ThreadRecordScope},
};
use aidash_domain::{
	ChildTaskSummary, TaskStatus, ThreadSnapshot,
	unassigned::{PendingChild, classify},
};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn record(
	scope: &mut dyn ThreadRecordScope,
	thread_id: Uuid,
	kind: &str,
	id: Uuid,
) -> Result<Value> {
	let resource = scope.thread(thread_id).await?;
	scope.require(&resource, "thread.read").await?;
	let mut snapshot = ThreadSnapshot {
		thread: scope.thread_row(thread_id).await?,
		tasks: vec![],
		artifacts: vec![],
		events: vec![],
		messages: vec![],
		unconfirmed: vec![],
	};
	let value = match kind {
		"thread" if id == thread_id => json!(snapshot.thread.clone()),
		"thread" => return Err(scope.missing_record(kind, id).await?),
		"task" => {
			let Some(task) = scope.task_record(thread_id, id).await? else {
				return Err(scope.missing_record(kind, id).await?);
			};
			if !scope.task_visible(&task).await? {
				return Err(Error::Forbidden);
			}
			snapshot.tasks.push(task.clone());
			json!(task)
		}
		"artifact" => {
			let Some(artifact) = scope.artifact_record(thread_id, id).await? else {
				return Err(scope.missing_record(kind, id).await?);
			};
			if !scope.artifact_visible(&artifact).await? {
				return Err(Error::Forbidden);
			}
			snapshot.artifacts.push(artifact.clone());
			json!(artifact)
		}
		"message" => {
			let Some(message) = scope.message_record(thread_id, id).await? else {
				return Err(scope.missing_record(kind, id).await?);
			};
			if !scope.message_visible(&message).await? {
				return Err(Error::Forbidden);
			}
			let attachments = scope.message_attachments(thread_id, &[id]).await?;
			snapshot.messages.push(message.clone());
			let mut value = json!(message);
			value["attachments"] = json!(
				attachments
					.into_iter()
					.filter(|row| row.message_id == id)
					.map(|row| row.attachment)
					.collect::<Vec<_>>()
			);
			value
		}
		"attachment" => {
			let Some(attachment) = scope.attachment_record(thread_id, id).await? else {
				return Err(scope.missing_record(kind, id).await?);
			};
			let message = scope
				.message_record(thread_id, attachment.message_id)
				.await?
				.ok_or(Error::Forbidden)?;
			if !scope.message_visible(&message).await? {
				return Err(Error::Forbidden);
			}
			snapshot.messages.push(message);
			json!({"attachment": attachment.attachment, "message_id": attachment.message_id, "sha256": attachment.sha256})
		}
		"event" => {
			if !scope.decide(&resource, "thread.events").await? {
				return Err(Error::Forbidden);
			}
			let Some(event) = scope.event_record(thread_id, id).await? else {
				return Err(scope.missing_record(kind, id).await?);
			};
			if !scope.event_visible(&event).await? {
				return Err(Error::Forbidden);
			}
			snapshot.events.push(event.clone());
			json!(event)
		}
		_ => return Err(Error::Invalid("unknown thread record kind".into())),
	};
	scope.track(&snapshot).await?;
	Ok(value)
}
/// Recheck that every message is still readable under current authority,
/// applying the same policy as `record` without disclosing or tracking a read.
pub async fn messages_readable(
	scope: &mut dyn ThreadRecordScope,
	thread_id: Uuid,
	ids: impl IntoIterator<Item = Uuid> + Send,
) -> Result<bool> {
	let resource = scope.thread(thread_id).await?;
	match scope.require(&resource, "thread.read").await {
		Ok(()) => {}
		Err(Error::Forbidden | Error::Unauthorized) => return Ok(false),
		Err(error) => return Err(error),
	}
	for id in ids {
		let Some(message) = scope.message_record(thread_id, id).await? else {
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
	thread_id: Uuid,
	parent_id: Uuid,
) -> Result<ChildTaskSummary> {
	let resource = scope.thread(thread_id).await?;
	scope.require(&resource, "thread.read").await?;
	let mut summary = ChildTaskSummary::default();
	// Which pending child is unassigned depends on its siblings, which may sit
	// on a later page: classify once, after the last page.
	let mut pending = Vec::new();
	let mut after = None;
	loop {
		let rows = scope.children(thread_id, parent_id, after).await?;
		let exhausted = rows.len() < 100;
		let mut visible_ids = Vec::with_capacity(rows.len());
		for row in rows {
			after = Some(row.id);
			scope.begin_row();
			if !scope
				.visible(&resource, thread_id, row.id, &row.created_by)
				.await?
			{
				// A child that cannot be confirmed is not "no pending child": the
				// parent would complete over a Task that may be unfinished or
				// failed. It is retried like a Read Recheck (#770).
				// Only a child whose status could change the summary matters: a
				// completed one cannot make the parent wait or fail. Any unfinished
				// child can change the open count and unassigned classification, even
				// when another pending child is already visible.
				if let Some(node) = scope.take_row_silence()
					&& !matches!(row.status, TaskStatus::Completed | TaskStatus::Abandoned)
				{
					return Err(Error::NodeUnreachable {
						node,
						after_effect: false,
					});
				}
				continue;
			}
			visible_ids.push(row.id);
			summary.include_task(row.id, row.status, row.revision);
			if !matches!(row.status, TaskStatus::Completed | TaskStatus::Abandoned) {
				pending.push(PendingChild {
					id: row.id,
					title: row.title,
					status: row.status,
					dependencies: row.dependencies,
					abandonable: row.abandonable,
					started: row.started,
				});
			}
		}
		scope.track_tasks(thread_id, &visible_ids).await?;
		if exhausted {
			summary.unassigned = classify(&pending);
			return Ok(summary);
		}
	}
}
#[cfg(test)]
mod tests;
