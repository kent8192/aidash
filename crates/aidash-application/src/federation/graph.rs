//! Authorized graph pagination and linkage shared by every transport.
use crate::{Error, Result, ports::graph::GraphProjectionScope};
use aidash_domain::federation::graph::*;
use serde_json::json;
use std::collections::BTreeSet;
pub fn checked_cursor(
	cursor: GraphCursor,
	binding: &str,
	generation: &str,
	now: i64,
) -> Result<GraphCursor> {
	if cursor.binding != binding
		|| cursor.kind >= CANDIDATE_KINDS
		|| cursor.window_end > now + 60
		|| cursor.expires_at - cursor.window_end != 300
	{
		return Err(Error::Forbidden);
	}
	if cursor.expires_at <= now || cursor.generation != generation {
		return Err(Error::Conflict("graph projection changed".into()));
	}
	Ok(cursor)
}
fn cursor_binding(node: &str, viewer: &GraphViewer, options: &GraphOptions) -> Result<String> {
	let mut scope = options.clone();
	scope.cursor = None;
	Ok(aidash_domain::registry::rules::digest(
		&json!({"node":node,"viewer":viewer,"scope":scope}),
	))
}
async fn linked_candidates(
	authority: &mut dyn GraphProjectionScope,
	candidate: &Candidate,
	options: &GraphOptions,
	node: &str,
	visible: &[GraphNode],
) -> Result<Vec<Candidate>> {
	let mut linked = Vec::new();
	let mut seen: BTreeSet<String> = visible.iter().map(|item| item.id.clone()).collect();
	for item in candidate.nodes(node, options) {
		seen.insert(item.id);
	}
	let mut workspace_ids = Vec::new();
	let mut task_ids = Vec::new();
	let mut registry = Vec::<(String, String, String)>::new();
	match candidate {
		Candidate::Workspace(_) => {}
		Candidate::Task(task) if options.scope_workspace.is_none() => {
			if (kind_allowed("workspace", options) || kind_allowed("goal", options))
				&& relation_allowed("contains", options)
			{
				workspace_ids.push(task.workspace_id);
			}
			if kind_allowed("task", options) {
				if relation_allowed("contains", options) {
					task_ids.extend(task.parent_id);
				}
				if relation_allowed("depends", options) {
					task_ids.extend(task.dependencies.iter().copied());
				}
			}
		}
		Candidate::Artifact(artifact) if options.scope_workspace.is_none() => {
			if kind_allowed("task", options) && relation_allowed("produces", options) {
				task_ids.push(artifact.task_id);
			}
		}
		Candidate::Run(run) if relation_allowed("executes", options) => {
			if options.scope_workspace.is_none() && kind_allowed("task", options) {
				task_ids.push(run.task_id);
			}
			if kind_allowed("agent", options) {
				registry.push((
					"agent".into(),
					run.agent_id.clone(),
					run.agent_version.clone(),
				));
			}
		}
		Candidate::Conversation(conversation) if options.scope_workspace.is_none() => {
			if kind_allowed("workspace", options) && relation_allowed("contains", options) {
				workspace_ids.push(conversation.workspace_id);
			}
			if relation_allowed("participates", options)
				&& kind_allowed(&conversation.target_kind, options)
				&& matches!(conversation.target_kind.as_str(), "agent" | "cluster")
				&& let Some((id, version)) = conversation.target.rsplit_once('@')
			{
				registry.push((conversation.target_kind.clone(), id.into(), version.into()));
			}
		}
		Candidate::Registry(entry) => {
			if entry.kind == "cluster"
				&& relation_allowed("coordinates", options)
				&& kind_allowed("agent", options)
				&& let Some((id, version)) = registry_ref(&entry.config["coordinator"])
			{
				registry.push(("agent".into(), id.into(), version.into()));
			}
			if entry.kind == "agent" {
				for (kind, field, relation) in [
					("tool", "tools", "tool"),
					("model", "model", "model"),
					("skill", "skills", "skill"),
					("cluster", "cluster", "member"),
				] {
					if !kind_allowed(kind, options) || !relation_allowed(relation, options) {
						continue;
					}
					let value = &entry.config[field];
					let refs = value
						.as_array()
						.map(|items| items.iter().collect::<Vec<_>>())
						.unwrap_or_else(|| vec![value]);
					for item in refs {
						if let Some((id, version)) = registry_ref(item) {
							registry.push((kind.into(), id.into(), version.into()));
						}
					}
				}
			}
		}
		_ => {}
	}
	task_ids.truncate(options.limit as usize);
	registry.truncate(options.limit as usize);
	for id in workspace_ids {
		if let Some(item) = authority.linked_workspace(id).await? {
			let new_nodes = item.nodes(node, options);
			if new_nodes.iter().any(|node| !seen.contains(&node.id))
				&& authority.visible(&item, false).await?
			{
				seen.extend(new_nodes.into_iter().map(|node| node.id));
				linked.push(item);
			}
		}
	}
	for id in task_ids {
		if seen.contains(&resource_key(node, "task", id)) {
			continue;
		}
		if let Some(item) = authority.linked_task(id).await?
			&& authority.visible(&item, false).await?
		{
			seen.extend(item.nodes(node, options).into_iter().map(|node| node.id));
			linked.push(item);
		}
	}
	for (kind, id, version) in registry {
		if seen.contains(&entity_key(node, &kind, &id, &version)) {
			continue;
		}
		if let Some(item) = authority.linked_registry(&kind, &id, &version).await?
			&& authority.visible(&item, false).await?
		{
			seen.extend(item.nodes(node, options).into_iter().map(|node| node.id));
			linked.push(item);
		}
	}
	Ok(linked)
}
pub async fn project(
	scope: &mut dyn GraphProjectionScope,
	source_node: &str,
	viewer: &GraphViewer,
	options: &GraphOptions,
) -> Result<GraphPage> {
	options.validate()?;
	let node_id = scope.node_id().to_owned();
	let generation = scope.generation(options).await?;
	let binding = cursor_binding(source_node, viewer, options)?;
	let mut cursor = match &options.cursor {
		Some(token) => checked_cursor(
			scope.decode_cursor(token)?,
			&binding,
			&generation,
			scope.now().timestamp(),
		)?,
		None => {
			let now = scope.now().timestamp();
			GraphCursor {
				kind: 0,
				offset: 0,
				generation: generation.clone(),
				binding: binding.clone(),
				window_end: now,
				expires_at: now + 300,
			}
		}
	};
	let mut nodes: Vec<GraphNode> = Vec::new();
	let mut records: Vec<Candidate> = Vec::new();
	let mut bytes = 0_usize;
	let mut scanned = 0_u64;
	let mut next_cursor = None;
	while cursor.kind < CANDIDATE_KINDS {
		if scanned >= 4096 {
			next_cursor = Some(scope.encode_cursor(&cursor)?);
			break;
		}
		let batch = scope
			.candidates(cursor.kind, cursor.offset, options)
			.await?;
		if batch.is_empty() {
			cursor.kind += 1;
			cursor.offset = 0;
			continue;
		}
		let exhausted = batch.len() < CANDIDATE_BATCH as usize;
		for candidate in batch {
			if scanned >= 4096 {
				next_cursor = Some(scope.encode_cursor(&cursor)?);
				break;
			}
			let before = cursor.offset;
			cursor.offset += 1;
			scanned += 1;
			let allowed_kind = kind_allowed(candidate.kind(), options)
				|| matches!(&candidate, Candidate::Workspace(_)) && kind_allowed("goal", options);
			if !allowed_kind {
				continue;
			}
			let visible = scope
				.visible(&candidate, options.scope_workspace.is_some())
				.await?;
			if !visible {
				continue;
			}
			if let Candidate::Run(run) = &candidate
				&& options.hours > 0
				&& run.phase().is_terminal()
				&& run.updated_at.timestamp() < cursor.window_end - i64::from(options.hours) * 3600
			{
				continue;
			}
			let addition: Vec<_> = candidate
				.nodes(&node_id, options)
				.into_iter()
				.filter(|node| !nodes.iter().any(|existing| existing.id == node.id))
				.collect();
			if addition.is_empty() {
				continue;
			}
			let mut group = addition;
			let mut group_records = vec![candidate.clone()];
			for linked in linked_candidates(scope, &candidate, options, &node_id, &nodes).await? {
				let addition: Vec<_> = linked
					.nodes(&node_id, options)
					.into_iter()
					.filter(|node| {
						!nodes
							.iter()
							.chain(group.iter())
							.any(|existing| existing.id == node.id)
					})
					.collect();
				if group.len() + addition.len() > options.limit as usize {
					continue;
				}
				group.extend(addition);
				group_records.push(linked);
			}
			let size = serde_json::to_vec(&group)?.len();
			if nodes.len() + group.len() > options.limit as usize || bytes + size > 3_000_000 {
				if nodes.is_empty() {
					return Err(Error::Invalid(
						"graph resource exceeds response limit".into(),
					));
				}
				cursor.offset = before;
				next_cursor = Some(scope.encode_cursor(&cursor)?);
				break;
			}
			bytes += size;
			nodes.extend(group);
			records.extend(group_records);
		}
		if next_cursor.is_some() {
			break;
		}
		if exhausted {
			cursor.kind += 1;
			cursor.offset = 0;
		}
	}
	let edges = build_edges(&node_id, &records, &nodes, options);
	let activity = scope.activity(options, &nodes, cursor.window_end).await?;
	if scope.generation(options).await? != generation {
		return Err(Error::Conflict("graph projection changed".into()));
	}
	let page = GraphPage {
		node_id,
		generation,
		checked_at: scope.now(),
		nodes,
		edges,
		activity,
		next_cursor,
	};
	if serde_json::to_vec(&page)?.len() > 3_000_000 {
		return Err(Error::Invalid("graph response exceeds size limit".into()));
	}
	Ok(page)
}

#[cfg(test)]
mod tests;
