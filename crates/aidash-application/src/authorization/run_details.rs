//! Inspection discloses persisted diagnostics only after the current Run visibility decision.
use crate::{
	Error, Result,
	ports::authorization::run_details::{RunDetails, RunDetailsRepository, RunDetailsScope},
};
use serde_json::json;
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
pub async fn inspect_in(
	scope: &mut dyn RunDetailsScope,
	id: Uuid,
	offset: u64,
) -> Result<RunDetails> {
	let raw = scope.run(id).await?.ok_or(Error::Forbidden)?;
	let run = raw.inspect();
	if run.home_node == scope.node_id() {
		let workspace = scope.workspace(run.workspace_id).await?;
		scope.set_context(workspace.attributes.clone());
		scope.require(&workspace, "workspace.read").await?;
	}
	if !scope.run_visible(&run).await? {
		return Err(Error::Forbidden);
	}
	let invocations = scope.invocations(id, offset).await?;
	let memory = scope.memory(&run.metadata).await?;
	let media_input_routes = scope.media_input_routes(&run.metadata).await?;
	Ok(RunDetails {
		run,
		invocations,
		memory: memory.unwrap_or_else(|| json!({})),
		media_input_routes,
	})
}
#[cfg(test)]
mod tests;
