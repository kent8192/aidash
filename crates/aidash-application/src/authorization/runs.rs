//! Resuming a Run retains the current subject scope and commits before waking workers.
//! Pausing and cancelling never depend on what the Run has read, so they have
//! their own check where a Subject's content-free management route answers them.
use crate::{
	Error, Result,
	ports::authorization::runs::{RunControlRepository, RunControlScope},
};
use aidash_domain::{RunInspection, run_state::RunReadsBlock};
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

/// Whether the caller is the subject the Run acts for.
async fn is_own_grant(scope: &mut dyn RunControlScope, id: Uuid) -> Result<bool> {
	let grant = scope
		.lock_execution_grant(id)
		.await?
		.ok_or(Error::Forbidden)?;
	let identity = scope.identity();
	Ok(grant.tenant == identity.tenant && grant.root_subject == identity.subject)
}

/// Only the subject the Run acts for may resume it.
async fn require_own_grant(scope: &mut dyn RunControlScope, id: Uuid) -> Result<()> {
	if is_own_grant(scope, id).await? {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

/// What the Run's reads say about letting it go on, for a caller who is not yet known to be
/// allowed to learn it. `None` when the Run is visible to the caller's policy.
async fn reads_block(
	scope: &mut dyn RunControlScope,
	run: &RunInspection,
) -> Result<Option<RunReadsBlock>> {
	if scope.run_visible(run).await? {
		return Ok(None);
	}
	scope
		.run_reads_block(run)
		.await?
		.ok_or(Error::Forbidden)
		.map(Some)
}

/// A Run whose Binding names a definition that may no longer be used would be refused at its
/// next worker boundary: say so now instead of waking it (#769). A Run that has ended, or whose
/// cancellation is requested, is not woken whatever it uses, and the control itself answers that.
/// Resuming and declining both let the Run go on, so both ask it (#763).
async fn use_block(
	scope: &mut dyn RunControlScope,
	run: &RunInspection,
) -> Result<Option<RunReadsBlock>> {
	let open = !run.phase.is_terminal() && run.control != aidash_domain::RunControl::Cancelled;
	if !open {
		return Ok(None);
	}
	scope.use_block(run).await
}

/// A paused Run that stopped because something it read changed (`invalidated`). Its context
/// already holds the obsolete text, so no resume can let it go on: only a Follow-up Task can
/// (#127). Its disclosure is withheld for the same reason, so this is asked before the read walk.
fn invalidated(run: &RunInspection) -> bool {
	run.control == aidash_domain::RunControl::Paused
		&& !run.phase.is_terminal()
		&& run.recovery.as_ref().and_then(|r| r.semantic_reason)
			== Some(aidash_domain::semantic::Failure::Invalidated)
}

pub async fn resume_in(scope: &mut dyn RunControlScope, id: Uuid) -> Result<RunInspection> {
	let run = in_workspace(scope, id).await?;
	let invalidated = invalidated(&run);
	let block = if invalidated {
		None
	} else {
		reads_block(scope, &run).await?
	};
	// Only someone who could otherwise resume the Run learns what blocks it.
	require_control(scope, id).await?;
	require_own_grant(scope, id).await?;
	if invalidated {
		return Err(Error::ResumeBlocked(RunReadsBlock::SourceChanged));
	}
	if let Some(block) = block {
		return Err(Error::ResumeBlocked(block));
	}
	if let Some(block) = use_block(scope, &run).await? {
		return Err(Error::ResumeBlocked(block));
	}
	let credential = scope.identity().credential_id;
	scope.update_credential(id, credential).await?;
	scope.resume_run(id).await
}

/// The longest key a person can name a call by; invocation keys are far shorter.
const CALL_KEY_LIMIT: usize = 256;

/// A person answers the call a Run is paused on, so the Run goes on without it (#763).
pub async fn decline_call(
	repository: &dyn RunControlRepository,
	id: Uuid,
	call: &str,
) -> Result<RunInspection> {
	let mut scope = repository.begin().await?;
	let result = decline_call_in(scope.as_mut(), id, call).await;
	let run = scope.finish(result).await?;
	repository.notify();
	Ok(run)
}

/// The Run's person (under the same check as resuming) or a holder of `run.decline_call` may
/// decline. Declining lets the Run go on, so it checks what the Run read, as resuming does. The
/// Run's person also has to be able to read the Run; someone else does not.
pub async fn decline_call_in(
	scope: &mut dyn RunControlScope,
	id: Uuid,
	call: &str,
) -> Result<RunInspection> {
	if call.is_empty() || call.len() > CALL_KEY_LIMIT {
		return Err(Error::Invalid("call key is empty or too long".into()));
	}
	let run = in_workspace(scope, id).await?;
	// A Run another Node executes is declined from its Home, never by this Node's people.
	if !run.is_local(scope.node_id()) {
		return Err(Error::Forbidden);
	}
	let own = is_own_grant(scope, id).await?;
	let block = if own {
		require_control(scope, id).await?;
		// The Run's person is held to what resuming checks, including reading the Run.
		reads_block(scope, &run).await?
	} else {
		let resource = scope.resource("run", &id.to_string(), json!({}));
		scope.require(&resource, "run.decline_call").await?;
		// Anyone else needs the Workspace and the action, not the Run's content (a Working Area
		// is private to its owner): only what the Run read has to be readable (#763).
		scope.run_reads_stop(&run).await?
	};
	if let Some(block) = block {
		return Err(Error::ResumeBlocked(block));
	}
	if let Some(block) = use_block(scope, &run).await? {
		return Err(Error::ResumeBlocked(block));
	}
	// Only the Run's person takes the Run over; anyone else's decline leaves its credential.
	if own {
		let credential = scope.identity().credential_id;
		scope.update_credential(id, credential).await?;
	}
	scope.decline_call(id, call).await
}
#[cfg(test)]
mod tests;
