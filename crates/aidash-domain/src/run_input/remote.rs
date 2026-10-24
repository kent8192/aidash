//! Remote corrections retain their original workspace and qualified input identity.
use crate::{Error, Message, Result, RunMetadata};

pub fn full_key(node: &str, task: uuid::Uuid, key: &str) -> String {
	format!("{node}:{task}:{key}")
}

pub fn historical_batch(
	node: &str,
	run: &RunMetadata,
	messages: Vec<Message>,
) -> Result<Vec<(String, Message)>> {
	let prefix = full_key(node, run.task_id, "");
	let mut batch = Vec::with_capacity(messages.len());
	for message in messages {
		let key = message
			.idempotency_key
			.as_deref()
			.and_then(|key| key.strip_prefix(&prefix))
			.ok_or_else(|| Error::Conflict("historical run message key changed".into()))?;
		if message.workspace_id != run.workspace_id {
			return Err(Error::Conflict(
				"historical run message workspace changed".into(),
			));
		}
		batch.push((key.to_owned(), message));
	}
	batch.sort_by_key(|(_, message)| (message.created_at, message.id));
	Ok(batch)
}

pub fn validate_delivery(
	node: &str,
	run: &RunMetadata,
	key: &str,
	content: &str,
	message: &Message,
) -> Result<()> {
	if message.workspace_id != run.workspace_id
		|| message.content != content
		|| message.idempotency_key.as_deref() != Some(full_key(node, run.task_id, key).as_str())
	{
		return Err(Error::Conflict(
			"remote run message delivery changed".into(),
		));
	}
	Ok(())
}

/// A preceding peer exposes a snapshot instead of the dedicated history endpoint.
pub fn legacy_history(node: &str, run: &RunMetadata, mut messages: Vec<Message>) -> Vec<Message> {
	let prefix = full_key(node, run.task_id, "");
	let run_id = run.id.to_string();
	let human = format!("human:{run_id}:");
	messages.retain(|message| {
		message
			.idempotency_key
			.as_deref()
			.and_then(|key| key.strip_prefix(&prefix))
			.is_some_and(|key| {
				key.starts_with(&human)
					|| (key.starts_with("subject-human:")
						&& key.rsplit(':').nth(1) == Some(run_id.as_str()))
			})
	});
	messages.sort_by_key(|message| (message.created_at, message.id));
	messages
}

#[cfg(test)]
mod tests;
