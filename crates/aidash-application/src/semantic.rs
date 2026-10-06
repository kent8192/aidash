//! Index sources only under restored current authority; keep cleanup identities durable.
use crate::{
	Error, Result,
	ports::{VectorIndex, semantic::*},
};
use aidash_domain::semantic::indexing::{self, IndexingSpec};
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
	let mut scope = match repository.restore(&authority).await {
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
	let text = if permitted && spec.enabled {
		scope.source(workspace, &source).await?
	} else {
		None
	};
	let Some(text) = text else {
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
	let digest = indexing::content_digest(&text);
	let old = scope.point(entry.point_id).await?;
	if indexing::has_current_point(&entry, &old, &digest)
		&& vector
			.present(&spec.vector, &plan.collection, &[entry.point_id])
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
        indexing::validate_text(&text, spec.max_input_bytes)?;
        vector.ensure_collection(&spec.vector, &plan.collection, spec.embedding.dimensions).await?;
        let embedding = scope.embed(workspace, &spec.embedding, &text, id).await?;
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
