//! Committed ownership and expiry drive reclamation; revoked users gain no read authority.
use crate::{Error, Result, ports::capabilities::reclamation::ReclamationRepository};
use aidash_domain::capabilities::{
	operations::MountedFile as FileEntry, reclamation::provisional_kind,
};
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn reclaim(repository: &dyn ReclamationRepository, id: Uuid) -> Result<()> {
	let mut scope = repository.begin().await?;
	let result = async {
		let mut record = scope.load(id).await?;

		let expired = record.expires_at.is_some_and(|at| at <= Utc::now());
		let mut files = vec![];
		match record.kind.as_str() {
			"transfer_out" => {
				if record.data["objects_released"] == true
					|| !(expired || matches!(record.state.as_str(), "delivered" | "blocked"))
				{
					return Ok(false);
				}
				files = serde_json::from_value::<Vec<FileEntry>>(
					record.data["description"]["files"].clone(),
				)?;
				if expired && !matches!(record.state.as_str(), "delivered" | "blocked") {
					record.state = if record.data["commit_attempted"] == true {
						"expired_unconfirmed"
					} else {
						"expired"
					}
					.into();
				}
				record.data["objects_released"] = json!(true);
			}
			"transfer_in" => {
				if record.data["objects_released"] == true
					|| !(expired || record.state == "committed")
				{
					return Ok(false);
				}
				for value in record.data["chunks"]
					.as_object()
					.ok_or(Error::Forbidden)?
					.values()
				{
					files.push(serde_json::from_value::<FileEntry>(value.clone())?);
				}
				let reserved = record.data["reserved"].as_i64().ok_or(Error::Forbidden)?;
				if reserved > 0 {
					scope.release_quota(&record.tenant, reserved).await?;
				}
				if record.state != "committed" {
					record.state = "expired".into();
				}
				record.data["reserved"] = json!(0);
				record.data["chunks"] = json!({});
				record.data["objects_released"] = json!(true);
			}
			"reference" => {
				let removing = record.state == "revoked"
					|| (expired
						&& matches!(record.state.as_str(), "uploading" | "extracting" | "failed"));
				if removing
					&& !record.data["operation_id"].is_null()
					&& !record.data["instance"].is_null()
					&& record.data["runner_released"] != true
				{
					let operation: Uuid =
						serde_json::from_value(record.data["operation_id"].clone())?;
					let observed = scope
						.request("POST", &format!("/v1/operations/{operation}/cancel"), None)
						.await?;
					let undispatched_absent =
						observed["status"] == "absent" && record.data["dispatch_pending"] == true;
					if observed["termination_confirmed"] != true && !undispatched_absent {
						return Err(Error::Conflict("EXTRACTOR_STOP_PENDING".into()));
					}
					if undispatched_absent {
						// This committed intent was never submitted to the runner, so a
						// verified absence is terminal and needs no acknowledgement.
						record.data["dispatch_pending"] = json!(false);
					} else {
						let original: FileEntry =
							serde_json::from_value(record.data["original"].clone())?;
						let digest = aidash_domain::registry::rules::digest(&json!([
							"extract/1",
							id,
							original
						]));
						scope
							.request(
								"POST",
								&format!("/v1/operations/{operation}/ack"),
								Some(json!({"digest":digest})),
							)
							.await?;
					}
					record.data["runner_released"] = json!(true);
				}
				if !removing && !matches!(record.state.as_str(), "ready" | "extracting") {
					return Ok(false);
				}
				for chunk in record.data["chunks"].as_array().ok_or(Error::Forbidden)? {
					files.push(serde_json::from_value::<FileEntry>(chunk["file"].clone())?);
				}
				record.data["chunks"] = json!([]);
				if removing {
					for key in ["original", "extraction"] {
						if let Some(file) =
							serde_json::from_value::<Option<FileEntry>>(record.data[key].clone())?
						{
							files.push(file);
						}
						record.data[key] = Value::Null;
					}
					record.state = if record.state == "revoked" {
						"revoked"
					} else {
						"expired"
					}
					.into();
					record.expires_at = None;
					record.data["objects_released"] = json!(true);
				}
			}
			_ => return Err(Error::Forbidden),
		}
		for file in files {
			// A transfer/reference record may delete its own provisional objects
			// only. Final receiver-area objects never match this guard.
			let kind = scope.object_kind(file.file_id, &record.tenant).await?;
			if let Some(kind) = kind {
				let expected = provisional_kind(&record.kind, &kind);
				if !expected {
					return Err(Error::Forbidden);
				}
				scope.erase(&record.tenant, file.file_id).await?;
			}
		}
		scope.update(&mut record).await?;
		Ok(true)
	}
	.await;
	scope.finish(result).await
}
async fn working_objects(repository: &dyn ReclamationRepository, cursor: &mut Uuid) -> Result<()> {
	let ids = repository.working(*cursor).await?;
	if ids.is_empty() {
		*cursor = Uuid::nil();
	}
	for id in ids {
		*cursor = id;
		let mut scope = repository.begin().await?;
		if let Some(tenant) = scope.working_tenant(id).await? {
			scope.erase(&tenant, id).await?;
		}
		scope.finish(Ok(true)).await?;
	}
	Ok(())
}
#[derive(Default)]
pub struct Cursor {
	python: Uuid,
	records: Uuid,
	working: Uuid,
}
pub async fn sweep(repository: &dyn ReclamationRepository, cursor: &mut Cursor) -> Result<()> {
	if let Err(error) = working_objects(repository, &mut cursor.working).await {
		tracing::warn!(%error,"superseded working bytes reclamation pending");
	}
	if let Err(error) = repository.python(&mut cursor.python).await {
		tracing::warn!(%error,"Python memory reclamation pending");
	}
	if let Err(error) = repository.orphans().await {
		tracing::warn!(%error,"object ownership reconciliation pending");
	}
	let ids = repository.retained(cursor.records).await?;
	if ids.is_empty() {
		cursor.records = Uuid::nil();
	}
	for id in ids {
		cursor.records = id;
		if let Err(error) = Box::pin(reclaim(repository, id)).await {
			tracing::warn!(%id,%error,"retained object reconciliation pending");
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
