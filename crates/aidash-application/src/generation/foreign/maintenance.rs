//! Foreign cancellation remains durable across lost RPCs and worker restarts.
use crate::{Error, Result, ports::generation::foreign::maintenance::ForeignGenerationMaintenance};
use aidash_domain::generation::{intent::Intent, requests::Request};
use uuid::Uuid;

pub async fn deliver_cancel(
	repository: &dyn ForeignGenerationMaintenance,
	id: Uuid,
	target: &str,
) -> Result<()> {
	let mut visibility = repository.begin_visibility().await?;
	let reserved = repository.reserve_retry(id).await?;
	visibility.suspend().await?;
	if reserved == 0 {
		return Ok(());
	}
	if !repository.send_cancel(target, id).await? {
		return Err(Error::Conflict(
			"remote generation cancellation was not acknowledged".into(),
		));
	}
	repository.mark_delivered(id).await
}

pub async fn terminate(
	repository: &dyn ForeignGenerationMaintenance,
	requested: &Request,
	status: &str,
) -> Result<()> {
	let mut scope = repository.begin_terminal(requested).await?;
	let result = async {
		let job = scope.load(&requested.tenant, requested.id).await?;
		if matches!(
			job.status.as_str(),
			"PENDING_APPROVAL" | "QUEUED" | "ACTIVE"
		) {
			scope
				.transition(
					&job,
					status,
					"generation-service",
					"foreign generation lifecycle reconciled",
				)
				.await?;
		}
		Ok(())
	}
	.await;
	scope.finish(result).await?;
	repository.notify();
	Ok(())
}

pub async fn cancel_at(
	repository: &dyn ForeignGenerationMaintenance,
	source: &str,
	id: Uuid,
) -> Result<bool> {
	if let Some(job) = repository.cancel_job(source, id).await? {
		terminate(repository, &job, "STOPPED").await?;
	}
	Ok(true)
}

pub async fn reconcile(repository: &dyn ForeignGenerationMaintenance) -> Result<()> {
	let mut visibility = repository.begin_visibility().await?;
	let pending = repository.pending().await?;
	visibility.suspend().await?;
	for (id, binding) in pending {
		let intent: Intent = serde_json::from_value(binding)?;
		if let Err(error) = deliver_cancel(repository, id, &intent.target_node).await {
			repository.warn_cancel(id, &error, true);
		}
	}
	// Every network wait ends before acquiring the next local visibility lease.
	let _visibility = repository.begin_visibility().await?;
	for job in repository.jobs().await? {
		let status = if job.expires_at <= repository.now() {
			"EXPIRED"
		} else if let Some(id) = job.admission_id {
			match repository.run_phase(id).await?.as_str() {
				"COMPLETED" => "COMPLETED",
				"CANCELLED" => "STOPPED",
				_ => "FAILED",
			}
		} else {
			"EXPIRED"
		};
		terminate(repository, &job, status).await?;
	}
	Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
