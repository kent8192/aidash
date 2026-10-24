//! Durable read provenance and remote-grant disclosure preserve all source requirements.
use crate::{
	Error, Result,
	ports::authorization::journals::{GrantJournalScope, ReadJournalScope, ReadMembership},
};
use aidash_domain::{WorkspaceSnapshot, registry::Entry, semantic::Failure};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;
pub async fn track_snapshot(
	scope: &mut dyn ReadJournalScope,
	snapshot: &WorkspaceSnapshot,
) -> Result<()> {
	let (read_run, read_grant) = scope.membership();
	let membership = match (read_run, read_grant) {
		(Some(run), None) => ReadMembership::Run(run),
		(None, Some(grant)) => ReadMembership::RemoteGrant(grant),
		(None, None) => return Ok(()),
		(Some(_), Some(_)) => return Err(Error::Forbidden),
	};
	let mut sources: BTreeSet<(String, Uuid)> = snapshot
		.tasks
		.iter()
		.map(|r| ("task".into(), r.id))
		.chain(snapshot.artifacts.iter().map(|r| ("artifact".into(), r.id)))
		.chain(snapshot.messages.iter().map(|r| ("message".into(), r.id)))
		.collect();
	let id = |value: &Value| value.as_str().and_then(|s| s.parse::<Uuid>().ok());
	if !snapshot.events.is_empty() {
		sources.insert(("workspace_events".into(), snapshot.workspace.id));
	}
	for event in &snapshot.events {
		let source = if event.kind.starts_with("conversation.") {
			id(&event.data["id"]).map(|id| ("conversation", id))
		} else if event.kind.starts_with("generation.") {
			id(&event.data["id"]).map(|id| ("generation", id))
		} else if event.kind == "run.created" {
			id(&event.data["id"]).map(|id| ("run", id))
		} else {
			id(&event.data["run_id"]).map(|id| ("run", id))
		};
		if let Some((kind, id)) = source
			&& (kind != "run" || Some(id) != read_run)
		{
			sources.insert((kind.into(), id));
		}
		if event.kind == "message.created" || event.kind == "message.thread_opened" {
			if let Some(id) = id(&event.data["id"]) {
				sources.insert(("message".into(), id));
			} else if event.kind == "message.created" {
				let ids = scope
					.legacy_messages(
						event.workspace_id,
						event.data["sender"].as_str(),
						event.data["content"].as_str(),
					)
					.await?;
				sources.extend(ids.into_iter().map(|id| ("message".into(), id)));
			}
		}
	}
	scope
		.record_sources(
			membership,
			snapshot.workspace.id,
			&sources.into_iter().collect::<Vec<_>>(),
		)
		.await
}
pub async fn track_registry(scope: &mut dyn ReadJournalScope, entries: &[Entry]) -> Result<()> {
	let (read_run, _) = scope.membership();
	let Some(run) = read_run else {
		return Ok(());
	};
	scope
		.record_registry(
			run,
			entries.iter().map(|entry| entry.id.clone()).collect(),
			entries.iter().map(|entry| entry.version.clone()).collect(),
		)
		.await
}
pub async fn grant_reads_visible(scope: &mut dyn GrantJournalScope, grant: Uuid) -> Result<bool> {
	match scope.remote_semantic_sources(grant).await {
		Ok(()) => {}
		Err(Error::Forbidden | Error::RemoteSemantic(Failure::Invalidated)) => return Ok(false),
		Err(error) => return Err(error),
	}
	let mut pending = vec![];
	for (workspace, kind, id) in scope.sources(grant).await? {
		if !scope
			.source_visible(workspace, &kind, id, &mut pending)
			.await?
		{
			return Ok(false);
		}
	}
	for run in pending {
		if !scope.run_reads(run).await? {
			return Ok(false);
		}
	}
	Ok(true)
}
#[cfg(test)]
mod tests;

/// Empty child pages do not need a new durable journal commit.
pub async fn track_tasks(
	scope: &mut dyn ReadJournalScope,
	workspace: Uuid,
	tasks: &[Uuid],
) -> Result<()> {
	if tasks.is_empty() {
		return Ok(());
	}
	let membership = match scope.membership() {
		(Some(run), None) => ReadMembership::Run(run),
		(None, Some(grant)) => ReadMembership::RemoteGrant(grant),
		(None, None) => return Ok(()),
		(Some(_), Some(_)) => return Err(Error::Forbidden),
	};
	scope
		.record_sources(
			membership,
			workspace,
			&tasks
				.iter()
				.map(|id| ("task".into(), *id))
				.collect::<Vec<_>>(),
		)
		.await
}
