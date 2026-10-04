//! Immutable manifest admission retains the caller's native transaction.
use crate::{
	Error, Result,
	ports::transactions::admission::{AdmissionRepository, AdmissionScope},
	registry::DefinitionValidation,
};
use aidash_domain::transactions::{
	Manifest,
	authority::{Origin, Status},
};
use serde_json::json;

pub async fn submit(
	repository: &dyn AdmissionRepository,
	validation: &DefinitionValidation,
	manifest: &Manifest,
	origin: Option<&Origin>,
) -> Result<Status> {
	let mut scope = repository.begin().await?;
	let stored = submit_in(scope.admission().as_mut(), validation, manifest, origin).await?;
	repository
		.fault(manifest.id, "coordinator.submit.before")
		.await?;
	scope.commit().await?;
	repository
		.fault(manifest.id, "coordinator.submit.after")
		.await?;
	repository.wake();
	Ok(stored)
}

pub async fn submit_in(
	scope: &mut dyn AdmissionScope,
	validation: &DefinitionValidation,
	manifest: &Manifest,
	origin: Option<&Origin>,
) -> Result<Status> {
	super::validate(validation, manifest)?;
	if manifest.coordinator != scope.node_id() {
		return Err(Error::Invalid("submit to the named coordinator".into()));
	}
	match scope.status(manifest.id).await {
		Ok(existing) => {
			scope.match_origin(manifest.id, origin).await?;
			if existing.digest != manifest.digest()? || existing.manifest != json!(manifest) {
				return Err(Error::Conflict(
					"transaction ID already has another immutable manifest".into(),
				));
			}
			return Ok(existing);
		}
		Err(Error::NotFound(_)) => {}
		Err(error) => return Err(error),
	}
	let remaining = manifest
		.deadline
		.signed_duration_since(scope.now())
		.num_seconds();
	if !(1..=3600).contains(&remaining) {
		return Err(Error::Invalid(
			"new transaction deadline must be within the next hour".into(),
		));
	}
	for node in &manifest.participants {
		if node.node_id != scope.node_id() {
			scope.resolve_peer(&node.node_id).await?;
			if !scope.trusted(&node.node_id).await? {
				return Err(Error::Forbidden);
			}
		}
	}
	let stored = scope.admit(manifest).await?;
	if let Some(origin) = origin {
		scope.bind_origin(manifest.id, origin).await?;
	}
	scope.match_origin(manifest.id, origin).await?;
	Ok(stored)
}

#[cfg(test)]
mod tests;
