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
	let agent: AgentConfig = serde_json::from_value(entry.config)?;
	if read_context && agent.core_capabilities.enabled() {
		scope.context_authority(run).await?;
	}
	// Immutable versions still require current tenant approval after an external wait.
	for reference in std::iter::once(&agent.model)
		.chain(agent.tools.iter())
		.chain(agent.skills.iter())
		.chain(agent.cluster.iter())
	{
		scope.catalog(reference, "registry.read").await?;
	}
	Ok(agent)
}
#[cfg(test)]
mod tests;
