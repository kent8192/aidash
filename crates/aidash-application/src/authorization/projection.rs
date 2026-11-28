//! Visibility is applied before pages, snapshots and fitted observations are disclosed.
use crate::{
	Result,
	ports::authorization::{projection::ThreadProjection, visibility::RunVerdicts},
};
use aidash_domain::{
	Event, Message, Task, ThreadSnapshot,
	threads::TaskPage,
	unconfirmed::{Kind, Tally},
};
use serde_json::Value;
use uuid::Uuid;
pub const EVENT_SCAN_LIMIT: usize = 4096;
pub async fn task_page(
	scope: &mut dyn ThreadProjection,
	threads: &[Uuid],
	offset: u64,
) -> Result<TaskPage> {
	let mut cursor = offset;
	let mut tasks: Vec<Task> = vec![];
	let mut unconfirmed = Tally::new();
	loop {
		let rows = scope.task_rows(threads, cursor).await?;
		let exhausted = rows.len() < 500;
		for task in rows {
			cursor = cursor.saturating_add(1);
			scope.begin_row();
			let visible = scope.task_visible(&task).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle(silence, Kind::Task, task.thread_id, visible) {
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
	scope: &mut dyn ThreadProjection,
	threads: &[Uuid],
	include_marketplace: bool,
) -> Result<Vec<Event>> {
	latest_visible_events_counted(scope, threads, include_marketplace)
		.await
		.map(|(events, _)| events)
}
/// The latest visible events and the events left out because a Node did not answer (#770).
pub async fn latest_visible_events_counted(
	scope: &mut dyn ThreadProjection,
	threads: &[Uuid],
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
			.event_rows(threads, include_marketplace, cursor, page_size)
			.await?;
		let exhausted = rows.len() < page_size;
		scanned += rows.len();
		for event in rows {
			cursor = event.sequence;
			scope.begin_row();
			let visible = scope.event_visible(&event, &mut runs).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle_in(silence, Kind::Event, event.thread_id, visible) {
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
	scope: &mut dyn ThreadProjection,
	thread: Uuid,
) -> Result<Vec<Message>> {
	latest_visible_messages_counted(scope, thread)
		.await
		.map(|(messages, _)| messages)
}
/// The latest visible messages and the messages left out because a Node did not answer (#770).
pub async fn latest_visible_messages_counted(
	scope: &mut dyn ThreadProjection,
	thread: Uuid,
) -> Result<(Vec<Message>, Tally)> {
	let mut offset = 0;
	let mut result = vec![];
	let mut unconfirmed = Tally::new();
	loop {
		let rows = scope.message_rows(thread, offset).await?;
		let exhausted = rows.len() < 100;
		for message in rows {
			scope.begin_row();
			let visible = scope.message_visible(&message).await?;
			let silence = scope.take_row_silence();
			if unconfirmed.settle(silence, Kind::Message, message.thread_id, visible) {
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
	scope: &mut dyn ThreadProjection,
	id: Uuid,
) -> Result<ThreadSnapshot> {
	let thread = scope.thread(id).await?;
	scope.require(&thread, "thread.read").await?;
	let mut unconfirmed = Tally::new();
	let events = if scope.decide(&thread, "thread.events").await? {
		let (events, left_out) = latest_visible_events_counted(scope, &[id], false).await?;
		unconfirmed.merge(left_out);
		events
	} else {
		vec![]
	};
	let thread_row = scope.thread_row(id).await?;
	let thread_tasks = scope.thread_tasks(id).await?;
	let (messages, left_out) = latest_visible_messages_counted(scope, id).await?;
	unconfirmed.merge(left_out);
	let mut snapshot = ThreadSnapshot {
		thread: thread_row,
		tasks: thread_tasks,
		messages,
		events,
		unconfirmed: vec![],
	};
	let mut tasks = vec![];
	for task in snapshot.tasks {
		scope.begin_row();
		let visible = scope.task_visible(&task).await?;
		let silence = scope.take_row_silence();
		if unconfirmed.settle(silence, Kind::Task, task.thread_id, visible) {
			tasks.push(task);
		}
	}
	snapshot.tasks = tasks;
	// Each message was judged by `latest_visible_messages` and is judged again
	// here. Messages dropped by the first pass are not in the list, so a message
	// dropped now, because a Node stopped answering in between, is counted once.
	let mut messages = vec![];
	for message in snapshot.messages {
		scope.begin_row();
		let visible = scope.message_visible(&message).await?;
		let silence = scope.take_row_silence();
		if unconfirmed.settle(silence, Kind::Message, message.thread_id, visible) {
			messages.push(message);
		}
	}
	snapshot.messages = messages;
	snapshot.unconfirmed = unconfirmed.into_entries();
	Ok(snapshot)
}
pub async fn snapshot(scope: &mut dyn ThreadProjection, id: Uuid) -> Result<ThreadSnapshot> {
	let snapshot = snapshot_untracked(scope, id).await?;
	scope.track(&snapshot).await?;
	Ok(snapshot)
}
pub async fn observation_fitted<F>(
	scope: &mut dyn ThreadProjection,
	id: Uuid,
	offset: usize,
	limit: usize,
	fits: F,
) -> Result<Option<(usize, Value)>>
where
	F: FnMut(usize, &Value) -> Result<bool>,
{
	fitted(scope, id, None, offset, limit, fits).await
}
/// `exclude_run` leaves that Run's own events out before the page is
/// projected, counted and tracked, so all three describe the same rows.
async fn fitted<F>(
	scope: &mut dyn ThreadProjection,
	id: Uuid,
	exclude_run: Option<Uuid>,
	offset: usize,
	limit: usize,
	fits: F,
) -> Result<Option<(usize, Value)>>
where
	F: FnMut(usize, &Value) -> Result<bool>,
{
	let mut snapshot = snapshot_untracked(scope, id).await?;
	if let Some(run) = exclude_run {
		aidash_domain::context::observation::without_run_events(&mut snapshot, run);
	}
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
	let dependencies = ThreadSnapshot {
		thread: snapshot.thread.clone(),
		tasks: snapshot
			.tasks
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
/// A Run's per-step Thread view passes its own id as `exclude_run` (#942);
/// the model's observe tool passes `None` and sees every event.
pub async fn observation(
	scope: &mut dyn ThreadProjection,
	id: Uuid,
	offset: usize,
	limit: usize,
	exclude_run: Option<Uuid>,
) -> Result<Value> {
	fitted(scope, id, exclude_run, offset, limit, |_, _| Ok(true))
		.await?
		.map(|(_, output)| output)
		.ok_or_else(|| crate::Error::Invalid("thread observation could not be fitted".into()))
}
