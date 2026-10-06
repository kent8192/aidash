//! Lifecycle decisions and retirement are shared by HTTP and recovery callers.
use crate::{
	Error, Result,
	ports::generation::lifecycle::{GenerationControls, GenerationLifecycleScope},
};
use aidash_domain::{
	entities::qualified_agent,
	generation::requests::{Action, Control, Request},
	identity::Principal,
};
use serde_json::json;
use uuid::Uuid;

pub async fn control(
	repository: &dyn GenerationControls,
	tenant: &str,
	id: Uuid,
	input: &Control,
) -> Result<Request> {
	let principal = repository.principal();
	if let Principal::Subject { tenant: caller, .. } = principal
		&& caller != tenant
	{
		return Err(Error::Forbidden);
	}
	let actor = match principal {
		Principal::Operator => "operator",
		Principal::Subject { subject, .. } => subject,
	};
	let mut scope = repository.begin(tenant).await?;
	let result = async {
		if matches!(principal, Principal::Operator) {
			scope.authority(tenant).await?;
		}
		let job = scope.load(tenant, id).await?;
		if matches!(principal, Principal::Subject { .. }) {
			if !scope.visible(&job).await? {
				return Err(Error::Forbidden);
			}
			if !scope.decide(&job, input.action.authorization()).await? {
				return Err(Error::Forbidden);
			}
		}
		control_in(scope.as_mut(), &job, input, actor).await
	}
	.await;
	let job = scope.finish(result).await?;
	repository.notify();
	Ok(job)
}
pub async fn control_in(
	scope: &mut dyn GenerationLifecycleScope,
	job: &Request,
	input: &Control,
	actor: &str,
) -> Result<Request> {
	aidash_domain::nonempty(&input.reason, "control reason")?;
	if input.reason.len() > 4096 {
		return Err(Error::Invalid("control reason exceeds 4096 bytes".into()));
	}
	let status = input.action.status();
	if scope.replay(job, status, actor, input).await? {
		return scope.load(&job.tenant, job.id).await;
	}
	if job.status == status {
		return Err(Error::Conflict(
			"generation decision already recorded".into(),
		));
	}
	if !input.action.accepts(job) {
		return Err(Error::Conflict(
			"invalid generation state transition".into(),
		));
	}
	if matches!(input.action, Action::Approve) && job.expires_at <= scope.now() {
		return Err(Error::Conflict("generation request expired".into()));
	}
	transition(scope, job, status, actor, &input.reason).await
}
pub async fn transition(
	scope: &mut dyn GenerationLifecycleScope,
	job: &Request,
	status: &str,
	actor: &str,
	reason: &str,
) -> Result<Request> {
	let terminal = matches!(
		status,
		"COMPLETED" | "DENIED" | "STOPPED" | "EXPIRED" | "FAILED" | "DELETED"
	);
	if terminal && !job.quota_released {
		let (unused, unused_calls, unused_embeddings) = scope.unused(job).await?;
		scope
			.release_policy(job, unused, unused_calls, unused_embeddings)
			.await?;
		scope.mark_quota_released(job).await?;
	}
	if matches!(status, "STOPPED" | "EXPIRED" | "DELETED") {
		scope.cancel_runs(job).await?;
	}
	if terminal {
		let mut snapshot = scope.authority(&job.tenant).await?;
		let subject = qualified_agent(scope.node_id(), &job.agent_id, &job.agent_version);
		if let Some(subject) = snapshot.bundle.subjects.get_mut(&subject)
			&& subject.enabled
		{
			subject.enabled = false;
			snapshot.revision = snapshot
				.revision
				.checked_add(1)
				.ok_or_else(|| Error::Invalid("authorization revision exhausted".into()))?;
			scope.save_authority(job, &snapshot, actor).await?;
		}
		if let Some(revision) = scope.retire_catalog(job).await? {
			// The exact retirement revision is the historical-read proof; a
			// later catalog change supersedes it even in this transaction.
			scope.record_retirement(job, revision, actor).await?;
		}
	}
	let updated = scope.update_status(job, status).await?;
	scope.history(job, status, actor, reason).await?;
	if job.home_node.is_empty() {
		scope
			.event(
				job.workspace_id,
				"generation.changed",
				json!({"id":job.id,"task_id":job.task_id,"policy_id":job.policy_id,"status":status}),
			)
			.await?;
	}
	Ok(updated)
}
#[cfg(test)]
mod tests;
