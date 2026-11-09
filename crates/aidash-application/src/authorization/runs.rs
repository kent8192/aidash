//! Resuming a Run retains the current subject scope and commits before waking workers.
//! Pausing and cancelling never depend on what the Run has read, so they have
//! their own check where a Subject's content-free management route answers them.
use crate::{
	Error, Result,
	ports::authorization::runs::{RunControlRepository, RunControlScope},
};
use aidash_domain::RunInspection;
use serde_json::json;
use uuid::Uuid;
pub async fn resume(repository: &dyn RunControlRepository, id: Uuid) -> Result<RunInspection> {
	let mut scope = repository.begin().await?;
	let result = resume_in(scope.as_mut(), id).await;
	let run = scope.finish(result).await?;
	repository.notify();
	Ok(run)
}
/// Whether the caller may control `id`, committing the authority read.
pub async fn authorize_run(
	repository: &dyn RunControlRepository,
	id: Uuid,
) -> Result<RunInspection> {
	let mut scope = repository.begin().await?;
	let result = authorize(scope.as_mut(), id).await;
	scope.finish(result).await
}

/// The current authority to control `id`: the Run is visible and `run.control`
/// is allowed. A caller that may stop its Live preview without controlling the
/// Run checks its own requester relation instead (ADR 0070).
pub async fn authorize(scope: &mut dyn RunControlScope, id: Uuid) -> Result<RunInspection> {
	let run = in_workspace(scope, id).await?;
	if !scope.run_visible(&run).await? {
		return Err(Error::Forbidden);
	}
	require_control(scope, id).await?;
	Ok(run)
}

async fn in_workspace(scope: &mut dyn RunControlScope, id: Uuid) -> Result<RunInspection> {
	let raw = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let run = raw.inspect();
	let workspace = scope.workspace(run.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	scope.require(&workspace, "workspace.read").await?;
	Ok(run)
}

async fn require_control(scope: &mut dyn RunControlScope, id: Uuid) -> Result<()> {
	let resource = scope.resource("run", &id.to_string(), json!({}));
	scope.require(&resource, "run.control").await
}

/// Only the subject the Run acts for may resume it.
async fn require_own_grant(scope: &mut dyn RunControlScope, id: Uuid) -> Result<()> {
	let grant = scope
		.lock_execution_grant(id)
		.await?
		.ok_or(Error::Forbidden)?;
	let identity = scope.identity();
	if grant.tenant != identity.tenant || grant.root_subject != identity.subject {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub async fn resume_in(scope: &mut dyn RunControlScope, id: Uuid) -> Result<RunInspection> {
	let run = in_workspace(scope, id).await?;
	if !scope.run_visible(&run).await? {
		let Some(block) = scope.run_reads_block(&run).await? else {
			return Err(Error::Forbidden);
		};
		// Only someone who could otherwise resume the Run learns what blocks it.
		require_control(scope, id).await?;
		require_own_grant(scope, id).await?;
		return Err(Error::ResumeBlocked(block));
	}
	require_control(scope, id).await?;
	require_own_grant(scope, id).await?;
	// A Run whose Binding names a definition that may no longer be used would be
	// refused at its next worker boundary: say so now instead of waking it (#769).
	// A Run that has ended, or whose cancellation is requested, is not woken whatever
	// it uses, and the resume itself answers that.
	let open = !run.phase.is_terminal() && run.control != aidash_domain::RunControl::Cancelled;
	if open && let Some(block) = scope.use_block(&run).await? {
		return Err(Error::ResumeBlocked(block));
	}
	let credential = scope.identity().credential_id;
	scope.update_credential(id, credential).await?;
	scope.resume_run(id).await
}
#[cfg(test)]
mod tests;
