//! Immutable manifest admission retains the caller's native transaction.
use crate::{
	Error, Result,
	ports::transactions::admission::{AdmissionRepository, AdmissionScope},
};
use aidash_domain::transactions::{
	Manifest,
	authority::{Origin, Status},
};
use serde_json::json;

pub async fn submit(
	repository: &dyn AdmissionRepository,
	manifest: &Manifest,
	origin: Option<&Origin>,
) -> Result<Status> {
	let mut scope = repository.begin().await?;
	let stored = submit_in(scope.admission().as_mut(), None, manifest, origin).await?;
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

/// Admit `manifest`. A new one is sealed with `seed`, or with a fresh seed
/// when the caller sent no preflight under one.
pub async fn submit_in(
	scope: &mut dyn AdmissionScope,
	seed: Option<&str>,
	manifest: &Manifest,
	origin: Option<&Origin>,
) -> Result<Status> {
	super::validate(manifest)?;
	if aidash_domain::nodes::deployment_of(&manifest.coordinator)
		!= aidash_domain::nodes::deployment_of(scope.node_id())
	{
		return Err(Error::Invalid("submit to the named coordinator".into()));
	}
	// The coordinator is a Node of this Deployment, and of the submitter's
	// Tenant whatever the participants are.
	let holder = scope.node_tenant(&manifest.coordinator).await?;
	match (holder, origin) {
		(None, _) => return Err(Error::Invalid("submit to the named coordinator".into())),
		(Some(holder), Some(origin)) if holder != origin.tenant => return Err(Error::Forbidden),
		_ => {}
	}
	match scope.status(manifest.id).await {
		Ok(existing) => {
			scope.match_origin(manifest.id, origin).await?;
			if existing.manifest != json!(manifest) {
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
		// The coordinator is this transaction's own Node.
		if node.node_id != manifest.coordinator
			&& !scope
				.trusted(&manifest.coordinator, &node.node_id, origin)
				.await?
		{
			return Err(Error::Forbidden);
		}
	}
	let fresh;
	let seed = match seed {
		Some(seed) => seed,
		None => {
			fresh = super::new_seed();
			&fresh
		}
	};
	let stored = scope.admit(manifest, seed).await?;
	if let Some(origin) = origin {
		scope.bind_origin(manifest.id, origin).await?;
	}
	scope.match_origin(manifest.id, origin).await?;
	Ok(stored)
}

#[cfg(test)]
mod tests;
