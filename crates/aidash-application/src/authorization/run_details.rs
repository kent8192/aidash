//! Inspection discloses persisted diagnostics only after the current Run visibility decision.
use crate::{
	Error, Result,
	ports::authorization::run_details::{RunDetails, RunDetailsRepository, RunDetailsScope},
};
use aidash_domain::RunInspection;
use uuid::Uuid;
pub async fn inspect(
	repository: &dyn RunDetailsRepository,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let mut scope = repository.begin().await?;
	let result = inspect_in(scope.as_mut(), id, offset).await;
	scope.finish(result).await
}
/// The same visibility decision as [`inspect`], without reading the Run's
/// persisted diagnostics. A missing or invisible Run is `Forbidden`.
pub async fn visible(repository: &dyn RunDetailsRepository, id: Uuid) -> Result<RunInspection> {
	let mut scope = repository.begin().await?;
	let result = visible_in(scope.as_mut(), id).await;
	scope.finish_visible(result).await
}
pub async fn visible_in(scope: &mut dyn RunDetailsScope, id: Uuid) -> Result<RunInspection> {
	let raw = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let run = raw.inspect();
	if run.is_local(scope.node_id()) {
		let workspace = scope.workspace(run.workspace_id).await?;
		scope.set_context(workspace.attributes.clone());
		scope.require(&workspace, "workspace.read").await?;
	}
	if !scope.run_visible(&run).await? {
		return Err(Error::Forbidden);
	}
	Ok(run)
}
pub async fn inspect_in(
	scope: &mut dyn RunDetailsScope,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let run = visible_in(scope, id).await?;
	let invocations = scope.invocations(id, offset).await?;
	let memory = scope.memory(&run.metadata).await?;
	let media_input_routes = scope.media_input_routes(&run.metadata).await?;
	Ok(RunDetails {
		provider_tool_activity: scope.provider_tool_activity(id, offset).await?,
		egress_requests: scope.egress_requests(id, offset).await?,
		run,
		invocations,
		memory,
		media_input_routes,
	})
}
#[cfg(test)]
mod tests;
