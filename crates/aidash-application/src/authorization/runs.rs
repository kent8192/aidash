//! Run controls retain the current subject scope and commit before waking workers.
use crate::{
	Error, Result,
	ports::authorization::runs::{RunControlRepository, RunControlScope},
};
use aidash_domain::{RunControlAction, RunInspection};
use serde_json::json;
use uuid::Uuid;
pub async fn control(
	repository: &dyn RunControlRepository,
	id: Uuid,
	action: RunControlAction,
) -> Result<RunInspection> {
	let mut scope = repository.begin().await?;
	let result = control_in(scope.as_mut(), id, action).await;
	let run = scope.finish(result).await?;
	repository.notify();
	Ok(run)
}
pub async fn control_in(
	scope: &mut dyn RunControlScope,
	id: Uuid,
	action: RunControlAction,
) -> Result<RunInspection> {
	let raw = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let run = raw.inspect();
	let workspace = scope.workspace(run.workspace_id).await?;
	scope.set_context(workspace.attributes.clone());
	scope.require(&workspace, "workspace.read").await?;
	if !scope.run_visible(&run).await? {
		return Err(Error::Forbidden);
	}
	let resource = scope.resource("run", &id.to_string(), json!({}));
	scope.require(&resource, "run.control").await?;
	if action == RunControlAction::Resume {
		let grant = scope
			.lock_execution_grant(id)
			.await?
			.ok_or(Error::Forbidden)?;
		let identity = scope.identity();
		if grant.tenant != identity.tenant || grant.root_subject != identity.subject {
			return Err(Error::Forbidden);
		}
		scope.update_credential(id, identity.credential_id).await?;
	}
	scope.control_run(id, action).await
}
#[cfg(test)]
mod tests;
