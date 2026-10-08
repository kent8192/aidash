//! Current authorization, activation and durable recovery apply outside HTTP too.
use crate::{
	Error, Result,
	generation::{policy, publication},
	ports::generation::provisioning::{GenerationActivationScope, GenerationProvisioning},
	registry::DefinitionValidation,
};
use aidash_domain::{
	generation::{policy::Spec, requests::Request},
	registry::{EntityRef, Entry},
};

pub async fn activate(
	repository: &dyn GenerationProvisioning,
	requested: &Request,
	validation: &DefinitionValidation,
) -> Result<()> {
	let mut scope = repository.begin_activation(requested).await?;
	let result = activate_in(scope.as_mut(), requested, validation).await;
	// A rejected admission must roll back its proposed policy revision. A denial
	// referencing that uncommitted revision cannot be retained independently;
	// reconciliation records the durable failure through a separate transaction.
	let result = result.map_err(|error| match error {
		Error::Forbidden => Error::Conflict("generated agent admission denied".into()),
		error => error,
	});
	scope.finish(result).await
}

pub async fn activate_in(
	scope: &mut dyn GenerationActivationScope,
	requested: &Request,
	validation: &DefinitionValidation,
) -> Result<()> {
	let job = scope.load(&requested.tenant, requested.id).await?;
	if job.status != "QUEUED" {
		return Ok(());
	}
	if job.expires_at <= scope.now() {
		return Err(Error::Conflict("generation request expired".into()));
	}
	scope.replace_subjects(job.subject_chain.clone());
	if !scope.visible(&job).await? {
		return Err(Error::Forbidden);
	}
	scope.require_request(&job).await?;
	let current = scope.current_policy(&job).await?;
	if !current.spec.enabled {
		return Err(Error::Forbidden);
	}
	let spec: Spec = serde_json::from_value(scope.pinned_policy(&job).await?)?;
	let _config = policy::validate(validation, &spec, &scope.snapshot().bundle)?;

	let snapshot = scope.bindings(&spec.template).await?;
	policy::validate_template_snapshot(&snapshot)?;
	for (reference, action) in snapshot
		.definitions
		.iter()
		.filter(|d| d.identity != snapshot.agent)
		.map(|d| {
			(
				d.identity.local(),
				crate::registry::bindings::component_action(&d.definition.kind),
			)
		})
		.chain(
			spec.compaction
				.iter()
				.map(|c| (c.provider.clone(), "compaction.invoke")),
		)
		.chain(
			spec.embedding
				.iter()
				.map(|c| (c.provider.clone(), "embedding.invoke")),
		) {
		scope.catalog_entry(&reference, "registry.read").await?;
		scope.catalog_entry(&reference, action).await?;
	}
	publication::publish(scope, &job, &spec).await?;
	let entry: Entry = serde_json::from_value(job.definition.clone())?;
	scope
		.transition(
			&job,
			"ACTIVE",
			"generation-service",
			"registered approved definition",
		)
		.await?;
	scope
		.delegate(
			job.task_id,
			&EntityRef {
				id: entry.id,
				version: entry.version,
			},
		)
		.await
}

pub async fn terminal(
	repository: &dyn GenerationProvisioning,
	requested: &Request,
	status: &str,
	reason: &str,
) -> Result<()> {
	// Do not release reserved quotas or retire the subject while bounded,
	// explicitly opted-in completion work still uses that exact live origin.
	// This occurs before acquiring authority/request locks used by retirement.
	if requested.status == "ACTIVE"
		&& requested.expires_at > repository.now()
		&& !repository.completion_ready(requested).await?
	{
		return Ok(());
	}
	let mut scope = repository.begin_terminal(requested).await?;
	let result = async {
		let job = scope.load(&requested.tenant, requested.id).await?;
		if matches!(
			job.status.as_str(),
			"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
		) {
			// A reconciler may have completed this Run while waiting for the lock.
			let phase = scope.run_phase(&job).await?;
			let terminal = match phase.as_deref() {
				Some("COMPLETED") => Some("COMPLETED"),
				Some("FAILED") => Some("FAILED"),
				Some("CANCELLED") => Some("STOPPED"),
				_ => None,
			};
			let (status, reason) = if let Some(status) = terminal {
				(status, "run reached terminal state")
			} else {
				(status, reason)
			};
			scope
				.transition(&job, status, "generation-service", reason)
				.await?;
		}
		Ok(())
	}
	.await;
	scope.finish(result).await
}

/// Recover a bounded batch while preserving remote-before-local and lock order.
pub async fn reconcile(
	repository: &dyn GenerationProvisioning,
	validation: &DefinitionValidation,
) -> Result<usize> {
	repository.dispatch_remote().await?;
	repository.reconcile_foreign().await?;
	let mut visibility = repository.begin_read().await?;
	let jobs = visibility.jobs().await?;
	let count = jobs.len();
	for job in jobs {
		if job.expires_at <= repository.now() {
			terminal(repository, &job, "EXPIRED", "generation lifetime elapsed").await?;
		} else if job.status == "QUEUED" {
			if let Err(error) = activate(repository, &job, validation).await {
				if admission_rejected(&error) {
					terminal(repository, &job, "FAILED", &error.to_string()).await?;
				} else {
					return Err(error);
				}
			}
		} else {
			terminal(repository, &job, "FAILED", "run reached terminal state").await?;
		}
	}
	if count > 0 {
		visibility.notify();
	}
	Ok(count)
}

fn admission_rejected(error: &Error) -> bool {
	matches!(
		error,
		Error::Unauthorized
			| Error::Forbidden
			| Error::Invalid(_)
			| Error::Conflict(_)
			| Error::Domain(aidash_domain::Error::Invalid(_) | aidash_domain::Error::Conflict(_))
	)
}

#[cfg(test)]
mod tests;
