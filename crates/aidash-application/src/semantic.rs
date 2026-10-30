//! Index sources only under restored current authority; keep cleanup identities durable.
use crate::{
	Error, Result,
	ports::{VectorIndex, semantic::*},
};
use aidash_domain::semantic::{
	EmbeddingInput, EmbeddingInputKind, Source,
	indexing::{self, IndexingSpec, UnsupportedReason},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn sweep(
	repository: &dyn SemanticIndexingRepository,
	vector: &dyn VectorIndex,
) -> Result<usize> {
	let visibility = repository.begin_visibility().await?;
	let ids = repository.due().await?;
	drop(visibility);
	let mut processed = 0;
	for id in ids {
		let _visibility = repository.begin_visibility().await?;
		if process(repository, vector, id).await? {
			processed += 1;
		}
	}
	cleanup(repository, vector).await?;
	repository.maintenance().await?;
	Ok(processed)
}

pub async fn process(
	repository: &dyn SemanticIndexingRepository,
	vector: &dyn VectorIndex,
	id: Uuid,
) -> Result<bool> {
	let Some((workspace, authority)) = repository.initial(id).await? else {
		return Ok(false);
	};
	let mut scope = match repository.restore(workspace, &authority).await {
		Ok(scope) => scope,
		Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => {
			repository.revoke(workspace, id, &authority).await?;
			return Ok(true);
		}
		Err(error) => return Err(error),
	};
	scope.durable();
	let result = index(scope.as_mut(), vector, workspace, id, &authority).await;
	scope.finish(result).await
}

enum IndexContent {
	Text(String),
	Image(IndexImage),
}

async fn index(
	scope: &mut dyn SemanticIndexingSession,
	vector: &dyn VectorIndex,
	workspace: Uuid,
	id: Uuid,
	authority: &Value,
) -> Result<bool> {
	let workspace_allowed = match scope.workspace_write(workspace).await {
		Ok(()) => true,
		Err(Error::Forbidden) => false,
		Err(error) => return Err(error),
	};
	let plan = scope.plan(workspace).await?;
	let Some(mut entry) = scope.lock_entry(id).await? else {
		return Ok(false);
	};
	if scope.authority(id).await? != *authority {
		return Ok(false);
	}
	let spec: IndexingSpec = serde_json::from_value(plan.spec.clone())?;
	let source = indexing::source(&entry)?;
	let permitted = workspace_allowed
		&& scope.permits("semantic.write").await?
		&& scope.permits("semantic.read").await?;
	let content = if permitted && spec.enabled {
		match &source {
			Source::WorkingImage { .. } => {
				let supported = spec.embedding.inputs.contains(&EmbeddingInputKind::Image);
				let limit = if supported { spec.max_image_bytes } else { 0 };
				let image = match scope.image(workspace, &source, limit).await {
					Ok(image) => image,
					Err(Error::Forbidden | Error::Unauthorized | Error::NotFound(_)) => None,
					Err(_) => {
						let (attempts, delay) = indexing::retry(entry.attempts);
						scope.failed(id, attempts, delay).await?;
						scope
							.history(
								workspace,
								id,
								entry.revision,
								"ERROR",
								"image source unavailable; durable retry scheduled",
							)
							.await?;
						return Ok(true);
					}
				};
				match image {
					None => None,
					Some(IndexImageRead::TooLarge) => {
						let reason = if supported {
							UnsupportedReason::TooLarge
						} else {
							UnsupportedReason::Modality
						};
						scope.unsupported(id, reason).await?;
						scope.retire_points(id).await?;
						scope
							.history(
								workspace,
								id,
								entry.revision,
								"UNSUPPORTED",
								reason.as_str(),
							)
							.await?;
						return Ok(true);
					}
					Some(IndexImageRead::Image(image))
						if !supported || image.bytes.len() > spec.max_image_bytes =>
					{
						let reason = if supported {
							UnsupportedReason::TooLarge
						} else {
							UnsupportedReason::Modality
						};
						scope.unsupported(id, reason).await?;
						scope.retire_points(id).await?;
						scope
							.history(
								workspace,
								id,
								entry.revision,
								"UNSUPPORTED",
								reason.as_str(),
							)
							.await?;
						return Ok(true);
					}
					Some(IndexImageRead::Image(image)) => Some(IndexContent::Image(image)),
				}
			}
			_ => scope
				.source(workspace, &source)
				.await?
				.map(IndexContent::Text),
		}
	} else {
		None
	};
	let Some(content) = content else {
		scope
			.set_revoked(
				id,
				if spec.enabled {
					"source authority revoked or source removed"
				} else {
					"semantic index disabled"
				},
			)
			.await?;
		scope.retire_points(id).await?;
		if entry.state != "REVOKED" {
			scope
				.history(
					workspace,
					id,
					entry.revision,
					"REVOKED",
					"indexing authority or source unavailable",
				)
				.await?;
		}
		return Ok(true);
	};
	let digest = match &content {
		IndexContent::Text(text) => indexing::content_digest(text),
		IndexContent::Image(image) => indexing::content_digest_bytes(&image.bytes),
	};
	let old = scope.point(entry.point_id).await?;
	if indexing::has_current_point(&entry, &old, &digest)
		&& vector
			.present(
				&spec.vector,
				&plan.collection,
				&plan.tenant,
				&[entry.point_id],
			)
			.await
			.unwrap_or(false)
	{
		scope.defer(id).await?;
		return Ok(true);
	}
	if indexing::needs_new_point(&old, &digest) {
		entry = scope.rotate(id, Uuid::new_v4()).await?;
		scope.schedule_point(&plan.collection).await?;
		scope
			.history(
				workspace,
				id,
				entry.revision,
				"PENDING",
				"linked source changed or authority restored",
			)
			.await?;
		// The fresh physical identity commits before any external write.
		return Ok(true);
	}
	let result = async {
        match &content {
            IndexContent::Text(text) => indexing::validate_text(text, spec.max_input_bytes)?,
            IndexContent::Image(image) => EmbeddingInput::Image { media_type: &image.media_type, bytes: &image.bytes }.validate(&spec.embedding)?,
        }
        vector.ensure_collection(&spec.vector, &plan.collection, spec.embedding.dimensions).await?;
        let embedding = match &content {
            IndexContent::Text(text) => scope.embed(workspace, &spec.embedding, text, id).await?,
            IndexContent::Image(image) => scope.embed_image(workspace, &spec.embedding, image, id).await?,
        };
        vector.upsert(&spec.vector, &plan.collection, entry.point_id, &embedding,
            json!({"entry_id":id,"revision":entry.revision,"index_revision":plan.revision,"workspace_id":workspace,"tenant":plan.tenant})).await
    }.await;
	match result {
		Ok(()) => {
			scope.record_digest(entry.point_id, &digest).await?;
			scope.ready(id).await?;
			scope
				.history(
					workspace,
					id,
					entry.revision,
					"READY",
					"vector write acknowledged",
				)
				.await?;
		}
		Err(_) => {
			let (attempts, delay) = indexing::retry(entry.attempts);
			scope.failed(id, attempts, delay).await?;
			scope
				.history(
					workspace,
					id,
					entry.revision,
					"ERROR",
					"indexing failed; durable retry scheduled",
				)
				.await?;
		}
	}
	Ok(true)
}

pub async fn cleanup(
	repository: &dyn SemanticIndexingRepository,
	vector: &dyn VectorIndex,
) -> Result<()> {
	let visibility = repository.begin_visibility().await?;
	let batch = repository.cleanup_due().await?;
	drop(visibility);
	// Each tombstone keeps its original lock through deletion and acknowledgement.
	// Visibility is released between independent effects.
	for id in batch.points {
		let _visibility = repository.begin_visibility().await?;
		let mut scope = repository.begin_cleanup().await?;
		if let Some((collection, config)) = scope.lock_point(id).await? {
			let failure = vector
				.delete_point(&config, &collection, id)
				.await
				.is_err()
				.then_some("vector deletion failed; retry scheduled");
			scope.record_point(id, failure).await?;
		}
		scope.commit().await?;
	}
	for collection in batch.collections {
		let _visibility = repository.begin_visibility().await?;
		let mut scope = repository.begin_cleanup().await?;
		if let Some(config) = scope.lock_collection(&collection).await? {
			let failure = vector
				.delete_collection(&config, &collection)
				.await
				.is_err()
				.then_some("collection deletion failed; retry scheduled");
			scope.record_collection(&collection, failure).await?;
		}
		scope.commit().await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests;

pub mod mutations;

pub mod retrieval;

pub mod embedding;

pub mod memory;

pub mod visibility;

pub mod remote_journal;

pub mod remote_status;
