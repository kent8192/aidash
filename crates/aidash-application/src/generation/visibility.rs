//! Generation disclosure uses the same task/workspace authority as workers.
use crate::{Result, ports::generation::visibility::GenerationVisibility};
use aidash_domain::{generation::requests::Request, policy::Resource};
use serde_json::json;

pub fn resource(scope: &dyn GenerationVisibility, job: &Request) -> Resource {
	scope.resource("generation", &job.id.to_string(), job.resource_attributes())
}
pub async fn visible(scope: &mut dyn GenerationVisibility, job: &Request) -> Result<bool> {
	if !job.home_node.is_empty() {
		if !scope.inherited_lease() {
			scope.context(json!({}));
		}
		let task = scope.resource(
			"task",
			&format!("{}/tasks/{}", job.home_node, job.task_id),
			json!({}),
		);
		return Ok(scope.decide(&task, "task.read").await?
			&& scope
				.decide(&resource(scope, job), "generation.read")
				.await?);
	}
	local_visible(scope, job).await
}
/// A persisted local Task remains authoritative in the native scoped reader.
pub async fn local_visible(scope: &mut dyn GenerationVisibility, job: &Request) -> Result<bool> {
	if !scope.inherited_lease() {
		scope.context(json!({}));
	}
	let workspace = scope.workspace(job.workspace_id).await?;
	let Some(task) = scope.task(job.task_id, job.workspace_id).await? else {
		return Ok(false);
	};
	if !scope.task_visible(&task).await? {
		return Ok(false);
	}
	scope.context(workspace.attributes.clone());
	Ok(scope.decide(&workspace, "workspace.read").await?
		&& scope
			.decide(&resource(scope, job), "generation.read")
			.await?)
}
#[cfg(test)]
mod tests;
