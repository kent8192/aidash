//! Mutations apply the same authority and replay rules for HTTP and worker callers.
use crate::{Error, Result, ports::semantic::mutations::*};
use aidash_domain::{
	semantic::{
		indexing::{IndexingSpec, validate_text},
		mutations::{self, Entry, History, Index, Put},
	},
	unconfirmed::{Kind, Tally, Unconfirmed},
};
use chrono::Utc;
use uuid::Uuid;

/// `acknowledged` confirms that disabling the index or changing its
/// embedding deletes or recomputes the Thread's Built-in Memory embeddings;
/// without it such a change is refused with the count it would affect (#1123).
pub async fn configure(
	repository: &dyn SemanticConfigurationRepository,
	thread: Uuid,
	revision: i64,
	spec: IndexingSpec,
	acknowledged: bool,
) -> Result<Index> {
	configure_index(
		repository,
		thread,
		revision,
		Configuration::Explicit(spec),
		acknowledged,
	)
	.await
}

pub async fn configure_for_tenant(
	repository: &dyn SemanticConfigurationRepository,
	thread: Uuid,
	revision: i64,
	options: IndexingSpec<()>,
	acknowledged: bool,
) -> Result<Index> {
	configure_index(
		repository,
		thread,
		revision,
		Configuration::Tenant(options),
		acknowledged,
	)
	.await
}

enum Configuration {
	Explicit(IndexingSpec),
	Tenant(IndexingSpec<()>),
}

async fn configure_index(
	repository: &dyn SemanticConfigurationRepository,
	thread: Uuid,
	revision: i64,
	configuration: Configuration,
	acknowledged: bool,
) -> Result<Index> {
	mutations::validate_revision(revision, "invalid index revision")?;
	let mut scope = repository.begin().await?;
	// Authority comes first: a caller without it learns neither whether the
	// Thread exists nor which private embedding origins are allowed.
	scope.authorize(thread).await?;
	let (tenant, old, spec) = match configuration {
		Configuration::Tenant(options) => {
			let tenant = scope.tenant(thread).await?;
			let active = scope.lock_active_embedding(&tenant).await?;
			scope.lock_thread(thread).await?;
			let old = scope.index(thread).await?;
			let embedding = match &old {
				// Preserve an installed generation until a Tenant switch publishes
				// its replacement, including indexes created before this setting.
				Some(index) => {
					serde_json::from_value::<IndexingSpec>(index.spec.clone())?.embedding
				}
				None => active,
			};
			let spec = options.with_embedding(embedding);
			repository.validate(&spec)?;
			(tenant, old, spec)
		}
		Configuration::Explicit(spec) => {
			repository.validate(&spec)?;
			scope.lock_thread(thread).await?;
			(
				scope.tenant(thread).await?,
				scope.index(thread).await?,
				spec,
			)
		}
	};
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
	if !acknowledged && let Some(old) = &old {
		let previous: IndexingSpec = serde_json::from_value(old.spec.clone())?;
		let removed = previous.enabled && !spec.enabled;
		let changed = previous.enabled && spec.enabled && previous.embedding != spec.embedding;
		if removed || changed {
			let facts = scope.memory_embeddings(thread).await?;
			if facts > 0 {
				return Err(Error::Conflict(format!(
					"this {} the embeddings of {facts} memory facts in this Thread; resend with acknowledge_memory_embeddings to continue",
					if removed { "deletes" } else { "recomputes" }
				)));
			}
		}
	}
	let (count, largest_memory) = scope.counts(thread).await?;
	mutations::validate_configuration_counts(&spec, count, largest_memory)?;
	let collection = format!("aidash_{}", Uuid::new_v4().simple());
	let current = scope
		.replace(thread, &tenant, revision + 1, value, &collection)
		.await?;
	scope.retire_collections(thread).await?;
	scope
		.record_collection(thread, &collection, serde_json::to_value(&spec.vector)?)
		.await?;
	for id in scope.active(thread).await? {
		let entry = scope.reconfigure(id, current.revision).await?;
		scope.schedule(&entry, &collection).await?;
	}
	scope.rebuild_images(thread, spec.max_images).await?;
	if spec.enabled {
		scope.backfill_memory(thread).await?;
	}
	scope.history(thread, current.revision).await?;
	scope.commit().await?;
	Ok(current)
}

pub async fn get_index(scope: &mut dyn SemanticEntriesSession, thread: Uuid) -> Result<Index> {
	scope.thread(thread, "semantic.read").await?;
	scope.index(thread, false).await
}

/// Public writes reserve the managed memory namespace before acquiring source authority.
pub fn validate_public_put(input: &Put) -> Result<()> {
	input.validate()?;
	if matches!(
		input.source,
		aidash_domain::semantic::Source::AttachmentImage { .. }
	) {
		return Err(Error::Invalid(
			"Thread Attachment images are indexed when they are posted".into(),
		));
	}
	if aidash_domain::semantic::indexing::is_managed_image_key(&input.key) {
		return Err(Error::Invalid(
			"image entry keys are managed by the Thread index".into(),
		));
	}
	if input.key.starts_with("agent-memory:")
		|| input.key.starts_with("memory-unit:")
		|| matches!(input.source, aidash_domain::semantic::Source::Unit { .. })
	{
		return Err(Error::Invalid(
			"native memory units are updated through identified memory mutations".into(),
		));
	}
	Ok(())
}

/// Inherited worker scopes can write their managed memory key through this same use case.
pub async fn put(
	scope: &mut dyn SemanticEntriesSession,
	thread: Uuid,
	input: Put,
) -> Result<Entry> {
	input.validate()?;
	scope.thread(thread, "semantic.write").await?;
	let index = scope.index(thread, true).await?;
	let spec = index.configuration()?;
	let old = scope.by_key(thread, &input.key).await?;
	let saved = scope.saved()?;
	let mut entry = Entry {
		id: old.as_ref().map_or_else(Uuid::new_v4, |old| old.id),
		thread_id: thread,
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
		if scope.count(thread).await? >= spec.max_sources as i64 {
			return Err(Error::Conflict("semantic source limit reached".into()));
		}
	}
	if !scope.permits(&entry, "semantic.write").await?
		|| !scope.permits(&entry, "semantic.read").await?
	{
		return Err(Error::Forbidden);
	}
	let text = scope
		.source(thread, &input.source)
		.await?
		.ok_or(Error::Forbidden)?;
	validate_text(&text, spec.max_input_bytes)?;
	let entry = scope.put(entry, saved).await?;
	scope.schedule(&entry, &index.collection).await?;
	scope
		.history(
			thread,
			entry.id,
			entry.revision,
			"PENDING",
			"source accepted",
		)
		.await?;
	Ok(entry)
}

/// The entries the reader may see, and how many were left out because a Node
/// did not answer (#770).
pub async fn entries(
	scope: &mut dyn SemanticEntriesSession,
	thread: Uuid,
) -> Result<(Vec<Entry>, Vec<Unconfirmed>)> {
	scope.thread(thread, "semantic.read").await?;
	scope.index(thread, false).await?;
	let mut visible = vec![];
	let mut unconfirmed = Tally::new();
	for entry in scope.list(thread).await? {
		scope.begin_row();
		if aidash_domain::semantic::retired_working_image(&entry.source)
			|| !scope.permits(&entry, "semantic.read").await?
		{
			let silence = scope.take_row_silence();
			unconfirmed.settle(silence, Kind::SemanticEntry, thread, false);
			continue;
		}
		let source: aidash_domain::semantic::Source = serde_json::from_value(entry.source.clone())?;
		let source_visible = if source.is_image() {
			scope.image_visible(thread, &source).await?
		} else if entry.deleted {
			true
		} else {
			scope.source(thread, &source).await?.is_some()
		};
		let silence = scope.take_row_silence();
		if unconfirmed.settle(silence, Kind::SemanticEntry, thread, source_visible) {
			visible.push(entry);
		}
	}
	Ok((visible, unconfirmed.into_entries()))
}

pub async fn change(
	scope: &mut dyn SemanticEntriesSession,
	thread: Uuid,
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
	scope.thread(thread, action).await?;
	let index = scope.index(thread, true).await?;
	let entry = scope
		.lock_entry(thread, id)
		.await?
		.ok_or(Error::Forbidden)?;
	if aidash_domain::semantic::retired_working_image(&entry.source) {
		return Err(Error::Forbidden);
	}
	if delete
		&& matches!(
			serde_json::from_value::<aidash_domain::semantic::Source>(entry.source.clone())?,
			aidash_domain::semantic::Source::Unit { .. }
		) {
		return Err(Error::Invalid(
			"delete native memory through a unit mutation with its observed revision".into(),
		));
	}
	if !scope.permits(&entry, action).await? || !scope.permits(&entry, "semantic.read").await? {
		return Err(Error::Forbidden);
	}
	if let aidash_domain::semantic::Source::AttachmentImage { .. } =
		serde_json::from_value(entry.source.clone())?
	{
		return Err(Error::Invalid(
			"Thread Attachment images are managed through their attachment".into(),
		));
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
			.source(thread, &serde_json::from_value(entry.source.clone())?)
			.await?
			.ok_or(Error::Forbidden)?;
		validate_text(&text, index.configuration()?.max_input_bytes)?;
	}

	let saved = scope.saved()?;
	let entry = scope
		.change(thread, id, Uuid::new_v4(), index.revision, delete, saved)
		.await?;
	scope.schedule(&entry, &index.collection).await?;
	scope
		.history(
			thread,
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

pub async fn history(scope: &mut dyn SemanticEntriesSession, thread: Uuid) -> Result<Vec<History>> {
	scope.thread(thread, "semantic.read").await?;
	let mut visible = vec![];
	let mut cursor = i64::MAX;
	loop {
		let rows = scope.history_page(thread, cursor).await?;
		let exhausted = rows.len() < 200;
		for row in rows {
			cursor = row.sequence;
			if let Some(id) = row.entry_id {
				let entry = scope.history_entry(id).await?;
				if aidash_domain::semantic::retired_working_image(&entry.source)
					|| !scope.permits(&entry, "semantic.read").await?
				{
					continue;
				}
				let source: aidash_domain::semantic::Source =
					serde_json::from_value(entry.source.clone())?;
				if source.is_image() && !scope.image_visible(thread, &source).await? {
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
