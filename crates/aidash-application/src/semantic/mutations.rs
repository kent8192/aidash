//! Mutations apply the same authority and replay rules for HTTP and worker callers.
use crate::{Error, Result, ports::semantic::mutations::*};
use aidash_domain::semantic::{
	indexing::{IndexingSpec, validate_text},
	mutations::{self, Entry, History, Index, Put},
};
use chrono::Utc;
use uuid::Uuid;

pub async fn configure(
	repository: &dyn SemanticConfigurationRepository,
	workspace: Uuid,
	revision: i64,
	spec: IndexingSpec,
) -> Result<Index> {
	repository.validate(&spec)?;
	mutations::validate_revision(revision, "invalid index revision")?;
	let mut scope = repository.begin().await?;
	scope.lock_workspace(workspace).await?;
	let tenant = scope.tenant(workspace).await?;
	let old = scope.index(workspace).await?;
	let value = serde_json::to_value(&spec)?;
	if old.as_ref().map_or(0, |old| old.revision) != revision {
		if let Some(old) = old
			&& old.revision == revision + 1
			&& old.spec == value
		{
			scope.commit().await?;
			return Ok(old);
		}
		return Err(Error::Conflict("semantic index revision changed".into()));
	}
	let (count, largest_memory) = scope.counts(workspace).await?;
	mutations::validate_configuration_counts(&spec, count, largest_memory)?;
	let collection = format!("aidash_{}", Uuid::new_v4().simple());
	let current = scope
		.replace(workspace, &tenant, revision + 1, value, &collection)
		.await?;
	scope.retire_collections(workspace).await?;
	scope
		.record_collection(workspace, &collection, serde_json::to_value(&spec.vector)?)
		.await?;
	for id in scope.active(workspace).await? {
		let entry = scope.reconfigure(id, current.revision).await?;
		scope.schedule(&entry, &collection).await?;
	}
	scope.history(workspace, current.revision).await?;
	scope.commit().await?;
	Ok(current)
}

pub async fn get_index(scope: &mut dyn SemanticEntriesSession, workspace: Uuid) -> Result<Index> {
	scope.workspace(workspace, "semantic.read").await?;
	scope.index(workspace, false).await
}

/// Public writes reserve the managed memory namespace before acquiring source authority.
pub fn validate_public_put(input: &Put) -> Result<()> {
	input.validate()?;
	if input.key.starts_with("agent-memory:") {
		return Err(Error::Invalid(
			"agent memory slots are updated through memory_write".into(),
		));
	}
	Ok(())
}

/// Inherited worker scopes can write their managed memory key through this same use case.
pub async fn put(
	scope: &mut dyn SemanticEntriesSession,
	workspace: Uuid,
	input: Put,
) -> Result<Entry> {
	input.validate()?;
	scope.workspace(workspace, "semantic.write").await?;
	let index = scope.index(workspace, true).await?;
	let spec = index.configuration()?;
	let old = scope.by_key(workspace, &input.key).await?;
	let saved = scope.saved()?;
	let mut entry = Entry {
		id: old.as_ref().map_or_else(Uuid::new_v4, |old| old.id),
		workspace_id: workspace,
		key: input.key,
		source: serde_json::to_value(&input.source)?,
		agent: input.agent,
		metadata: input.metadata,
		revision: input.expected_revision + 1,
		point_id: Uuid::new_v4(),
		index_revision: index.revision,
		deleted: false,
		state: "PENDING".into(),
		attempts: 0,
		last_error: None,
		created_by: saved["subject"].as_str().unwrap_or_default().into(),
		updated_at: Utc::now(),
	};
	if let Some(old) = &old {
		if !scope.permits(old, "semantic.write").await?
			|| !scope.permits(old, "semantic.read").await?
		{
			return Err(Error::Forbidden);
		}
		if old.deleted {
			return Err(Error::Conflict(
				"deleted semantic keys cannot be reused".into(),
			));
		}
		if !input
			.source
			.same_origin(&serde_json::from_value(old.source.clone())?)
		{
			return Err(Error::Conflict(
				"semantic source identity is immutable".into(),
			));
		}
		entry.created_by = old.created_by.clone();
		if mutations::put_replays(old, &entry, input.expected_revision) {
			return Ok(old.clone());
		}
		if old.revision != input.expected_revision {
			return Err(Error::Conflict("semantic source revision changed".into()));
		}
	} else {
		if input.expected_revision != 0 {
			return Err(Error::Conflict("semantic source does not exist".into()));
		}
		if scope.count(workspace).await? >= spec.max_sources as i64 {
			return Err(Error::Conflict("semantic source limit reached".into()));
		}
	}
	if !scope.permits(&entry, "semantic.write").await?
		|| !scope.permits(&entry, "semantic.read").await?
	{
		return Err(Error::Forbidden);
	}
	let text = scope
		.source(workspace, &input.source)
		.await?
		.ok_or(Error::Forbidden)?;
	validate_text(&text, spec.max_input_bytes)?;
	let entry = scope.put(entry, saved).await?;
	scope.schedule(&entry, &index.collection).await?;
	scope
		.history(
			workspace,
			entry.id,
			entry.revision,
			"PENDING",
			"source accepted",
		)
		.await?;
	Ok(entry)
}

pub async fn entries(
	scope: &mut dyn SemanticEntriesSession,
	workspace: Uuid,
) -> Result<Vec<Entry>> {
	scope.workspace(workspace, "semantic.read").await?;
	scope.index(workspace, false).await?;
	let mut visible = vec![];
	for entry in scope.list(workspace).await? {
		if scope.permits(&entry, "semantic.read").await?
			&& (entry.deleted
				|| scope
					.source(workspace, &serde_json::from_value(entry.source.clone())?)
					.await?
					.is_some())
		{
			visible.push(entry);
		}
	}
	Ok(visible)
}

pub async fn change(
	scope: &mut dyn SemanticEntriesSession,
	workspace: Uuid,
	id: Uuid,
	revision: i64,
	delete: bool,
) -> Result<Entry> {
	mutations::validate_revision(revision, "invalid source revision")?;
	let action = if delete {
		"semantic.delete"
	} else {
		"semantic.write"
	};
	scope.workspace(workspace, action).await?;
	let index = scope.index(workspace, true).await?;
	let entry = scope
		.lock_entry(workspace, id)
		.await?
		.ok_or(Error::Forbidden)?;
	if !scope.permits(&entry, action).await? || !scope.permits(&entry, "semantic.read").await? {
		return Err(Error::Forbidden);
	}
	if !delete
		&& !entry.deleted
		&& entry.revision == revision + 1
		&& scope.reindex_replay(id, entry.revision).await?
	{
		return Ok(entry);
	}
	if entry.deleted && delete && (entry.revision == revision || entry.revision == revision + 1) {
		return Ok(entry);
	}
	if entry.deleted || entry.revision != revision {
		return Err(Error::Conflict(
			"semantic source revision changed or deleted".into(),
		));
	}
	if !delete {
		let text = scope
			.source(workspace, &serde_json::from_value(entry.source.clone())?)
			.await?
			.ok_or(Error::Forbidden)?;
		validate_text(&text, index.configuration()?.max_input_bytes)?;
	} else if let Some((agent, version, home)) = scope.managed_memory(id).await? {
		scope
			.require_memory_write(workspace, &agent, &version)
			.await?;
		scope.delete_memory(workspace, agent, version, home).await?;
	}
	let saved = scope.saved()?;
	let entry = scope
		.change(workspace, id, Uuid::new_v4(), index.revision, delete, saved)
		.await?;
	scope.schedule(&entry, &index.collection).await?;
	scope
		.history(
			workspace,
			id,
			entry.revision,
			&entry.state,
			if delete {
				"source tombstoned"
			} else {
				"reindex requested"
			},
		)
		.await?;
	Ok(entry)
}

pub async fn history(
	scope: &mut dyn SemanticEntriesSession,
	workspace: Uuid,
) -> Result<Vec<History>> {
	scope.workspace(workspace, "semantic.read").await?;
	let mut visible = vec![];
	let mut cursor = i64::MAX;
	loop {
		let rows = scope.history_page(workspace, cursor).await?;
		let exhausted = rows.len() < 200;
		for row in rows {
			cursor = row.sequence;
			if let Some(id) = row.entry_id {
				let entry = scope.history_entry(id).await?;
				if !scope.permits(&entry, "semantic.read").await? {
					continue;
				}
			}
			visible.push(row);
			if visible.len() == 200 {
				break;
			}
		}
		if exhausted || visible.len() == 200 {
			break;
		}
	}
	Ok(visible)
}

#[cfg(test)]
mod tests;
