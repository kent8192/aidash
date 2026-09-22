//! Bounded model views. Durable snapshots and audit payloads remain unchanged.
use crate::{Error, Result, domain::WorkspaceSnapshot};
use serde_json::{Value, json};

pub(crate) const DEFAULT_LIMIT: usize = 20;
pub(crate) const MAX_LIMIT: usize = 50;

fn preview(text: &str, limit: usize) -> String {
	let mut chars = text.chars();
	let mut head: String = chars.by_ref().take(limit).collect();
	if chars.next().is_some() {
		head.push_str("… [use workspace_read]");
	}
	head
}

/// Collections are paged independently with a common offset. Events/messages
/// are newest first; tasks/artifacts retain snapshot order. Counts and cursors
/// describe this snapshot, so callers should refresh after concurrent changes.
pub(crate) fn project(snapshot: &WorkspaceSnapshot, offset: usize, limit: usize) -> Value {
	let limit = limit.clamp(1, MAX_LIMIT);
	let w = &snapshot.workspace;
	let mut value = json!({
		"view":"workspace_observation_v1",
		"workspace":{"id":w.id,"title":preview(&w.title,256),"goal":preview(&w.goal,512),"revision":w.revision},
		"tasks":snapshot.tasks.iter().skip(offset).take(limit).map(|t| json!({
			"id":t.id,"title":preview(&t.title,256),"description_preview":preview(&t.description,512),
			"status":t.status,"owner":t.owner,"parent_id":t.parent_id,"revision":t.revision,
			"dependencies":t.dependencies.iter().take(MAX_LIMIT).collect::<Vec<_>>(),
			"dependency_count":t.dependencies.len()
		})).collect::<Vec<_>>(),
		"artifacts":snapshot.artifacts.iter().skip(offset).take(limit).map(|a| json!({
			"id":a.id,"task_id":a.task_id,"kind":a.kind,"name":preview(&a.name,256),"created_at":a.created_at
		})).collect::<Vec<_>>(),
		"events":snapshot.events.iter().rev().skip(offset).take(limit).map(|e| json!({
			"id":e.id,"sequence":e.sequence,"kind":preview(&e.kind,128),"created_at":e.created_at
		})).collect::<Vec<_>>(),
		"messages":snapshot.messages.iter().rev().skip(offset).take(limit).map(|m| json!({
			"id":m.id,"sender":preview(&m.sender,256),"content_preview":preview(&m.content,512),"created_at":m.created_at
		})).collect::<Vec<_>>(),
		"details":"Use workspace_read with kind and id for full records. Concatenate its content chunks in offset order to recover the JSON record."
	});
	for (name, count) in [
		("tasks", snapshot.tasks.len()),
		("artifacts", snapshot.artifacts.len()),
		("events", snapshot.events.len()),
		("messages", snapshot.messages.len()),
	] {
		let end = offset.saturating_add(limit).min(count);
		value["pages"][name] = json!({"offset":offset,"limit":limit,"total":count,"next_offset":(end<count).then_some(end)});
	}
	value
}

pub(crate) fn chunk_record(
	value: Value,
	kind: &str,
	id: &str,
	offset: usize,
	max_chars: usize,
) -> Result<Value> {
	let id = id
		.parse::<uuid::Uuid>()
		.map_err(|_| Error::Invalid("invalid workspace record id".into()))?;
	let text = value.to_string();
	let total = text.chars().count();
	if offset > total {
		return Err(Error::Invalid(
			"workspace record offset out of range".into(),
		));
	}
	let content: String = text
		.chars()
		.skip(offset)
		.take(max_chars.min(16000))
		.collect();
	let end = offset + content.chars().count();
	Ok(json!({
		"kind":kind,"id":id,"encoding":"json","content":content,
		"offset":offset,"total_chars":total,"next_offset":(end<total).then_some(end),
		"budget_limited":max_chars == 0
	}))
}

/// Upgrade replayed legacy observations in the working context only. Human
/// records, other tool results and the durable invocation journal stay intact.
pub(crate) fn normalize_history(history: &mut [Value]) {
	for event in history {
		if event["kind"] == "tool"
			&& event["call"]["name"] == "workspace_observe"
			&& event["result"]["view"] != "workspace_observation_v1"
			&& let Ok(snapshot) =
				serde_json::from_value::<WorkspaceSnapshot>(event["result"].clone())
		{
			event["result"] = project(&snapshot, 0, DEFAULT_LIMIT);
		}
	}
}
