//! Source policy independently authorizes every actor and exact receiver dependency.
use crate::{Error, Result, ports::authorization::source::SourceAuthorityScope};
use aidash_domain::{Task, federation::execution::Inspection, policy::SubjectKind};
use serde_json::json;
pub async fn authorize<S: SourceAuthorityScope + ?Sized>(
	access: &mut S,
	task: &Task,
	node: &str,
	inspection: &Inspection,
) -> Result<()> {
	let executor =
		aidash_domain::agent_subject(node, &inspection.agent.id, &inspection.agent.version);
	let subjects = access.source_subjects().to_vec();
	if subjects.len() < 2
		|| subjects.last() != Some(&executor)
		|| access
			.source_bundle()
			.subjects
			.get(&executor)
			.is_some_and(|subject| subject.kind != SubjectKind::Agent)
	{
		return Err(Error::Forbidden);
	}
	access
		.generation_home(task, node, inspection.generation.as_ref())
		.await?;
	access.source_replace_subjects(subjects[..subjects.len() - 1].to_vec());
	let result = async {
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
			if aidash_domain::registry::bindings::defaults::is_compaction_decider(
				&definition.metadata,
			) && inspection.decider.as_ref() != Some(&definition.entry)
			{
				continue;
			}
			let action = match definition.kind.as_str() {
				"agent" => "agent.execute",
				"model" => "model.infer",
				"tool" => "tool.invoke",
				"skill" => "skill.use",
				"cluster" => "cluster.execute",
				"decider" => "decision.invoke",
				// Native provider roles are pinned configuration dependencies. Their
				// model/effect calls retain the separate runtime authorization checks.
				"source" | "memory" | "bundle" | "embedding" | "reranker" | "tokenizer"
				| "external_service" => continue,
				_ => return Err(Error::Forbidden),
			};
			access.source_require(&resource, action).await?;
		}
		Ok(())
	}
	.await;
	access.source_replace_subjects(subjects);
	result
}
#[cfg(test)]
mod tests;

pub mod grants;

pub mod semantic;

pub mod reads;

pub mod provenance;

pub mod search;

pub mod snapshot;
