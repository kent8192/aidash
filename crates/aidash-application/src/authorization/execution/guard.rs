//! Recheck current execution and every pinned dependency before accepting worker output.
use crate::{Error, Result, ports::authorization::guard::RunGuardScope};
use aidash_domain::{
	RunMetadata, qualified_agent,
	registry::{AgentConfig, EntityRef},
};

pub async fn authorize(
	scope: &mut dyn RunGuardScope,
	run: &RunMetadata,
	read_context: bool,
) -> Result<AgentConfig> {
	if !scope.run_visible(run).await? {
		return Err(Error::Forbidden);
	}
	// A conversation keeps its approved cluster even when its coordinator omits that cluster.
	for cluster in scope.cluster_targets(run.workspace_id).await? {
		let (id, version) = cluster.rsplit_once('@').ok_or(Error::Forbidden)?;
		scope
			.catalog(
				&EntityRef {
					id: id.into(),
					version: version.into(),
				},
				"cluster.execute",
			)
			.await?;
	}
	let task = scope.task_read(run.task_id).await?;
	let resource = scope.task_resource(&task).await?;
	scope.require(&resource, "task.execute").await?;
	let reference = EntityRef {
		id: run.agent_id.clone(),
		version: run.agent_version.clone(),
	};
	scope.require_live(run.task_id, &reference).await?;
	let entry = scope.catalog(&reference, "agent.execute").await?;
	super::require_agent(
		scope.bundle(),
		&qualified_agent(scope.node_id(), &run.agent_id, &run.agent_version),
	)?;
	scope.check_pinned(&entry).await?;
	if !read_context {
		// Failure delivery only closes the Task under current Task, Agent and
		// installation authority. It cannot infer, read Sources or invoke Tools.
		// A damaged inference context must not strand that durable obligation.
		return Ok(AgentConfig::from_definition(serde_json::from_value(
			entry.config,
		)?));
	}
	let snapshot = scope.binding_snapshot(run).await?;
	let agent = AgentConfig::from_snapshot(&snapshot)?;
	if read_context && agent.needs_context_authority() {
		scope.context_authority(run).await?;
	}
	// Current approval applies to every retained definition, independently of
	// the installation's newer active pointer.
	for saved in &snapshot.definitions {
		let current = scope
			.catalog(&saved.identity.local(), "registry.read")
			.await?;
		if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?) != saved.digest {
			return Err(Error::Conflict("admitted definition changed".into()));
		}
	}

	Ok(agent)
}
#[cfg(test)]
mod tests;
