//! Source policy independently authorizes every actor and exact receiver dependency.
use crate::{Error, Result, ports::authorization::source::SourceAuthorityScope};
use aidash_domain::{
	Task, federation::execution::Inspection, policy::SubjectKind, qualified_agent,
};
use serde_json::json;
pub async fn authorize<S: SourceAuthorityScope + ?Sized>(
	access: &mut S,
	task: &Task,
	node: &str,
	inspection: &Inspection,
) -> Result<()> {
	access
		.generation_home(task, node, inspection.generation.as_ref())
		.await?;
	let executor = qualified_agent(node, &inspection.agent.id, &inspection.agent.version);
	if access.source_subjects().last() != Some(&executor)
		|| access
			.source_bundle()
			.subjects
			.get(&executor)
			.is_none_or(|subject| subject.kind != SubjectKind::Agent)
	{
		return Err(Error::Forbidden);
	}
	let workspace = access.source_workspace(task.workspace_id).await?;
	access.source_context(workspace.attributes.clone());
	access.source_require(&workspace, "workspace.read").await?;
	let resource = access.source_task_resource(task).await?;
	access.source_require(&resource, "task.read").await?;
	access.source_require(&resource, "task.delegate").await?;
	access.source_require(&resource, "task.execute").await?;
	let resource = access.source_resource("node", node, json!({"remote_node":node}));
	access
		.source_require(&resource, "federation.execute")
		.await?;
	for definition in &inspection.definitions {
		let id = format!(
			"{node}/{}s/{}@{}",
			definition.kind, definition.entry.id, definition.entry.version
		);
		let mut resource = access.source_resource(
			&definition.metadata.kind,
			&definition.metadata.id,
			crate::authorization::catalog::attributes(&definition.metadata),
		);
		resource.id = id;
		resource.attributes["remote_node"] = json!(node);
		resource.attributes["digest"] = json!(definition.digest);
		access.source_require(&resource, "registry.read").await?;
		let action = match definition.kind.as_str() {
			"agent" => "agent.execute",
			"model" => "model.infer",
			"tool" => "tool.invoke",
			"skill" => "skill.use",
			"cluster" => "cluster.execute",
			"compactor" => "compaction.invoke",
			_ => return Err(Error::Forbidden),
		};
		access.source_require(&resource, action).await?;
	}
	Ok(())
}
#[cfg(test)]
mod tests;

pub mod grants;
