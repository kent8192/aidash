//! Recheck current execution and every pinned dependency before accepting worker output.
use crate::{Error, Result, ports::authorization::guard::RunGuardScope};
use aidash_domain::{
	RunMetadata,
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
	super::require_agent(scope.bundle(), &run.executor_subject(scope.node_id()))?;
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
	// Current local approval applies independently of a newer active pointer.
	// Foreign Agent closures are refreshed at their owning Node by the Binding
	// authority; never resolve their unqualified IDs through this local catalog.
	// A delegated agent's own dependencies are pinned here as metadata and are
	// restricted to that agent, who uses them directly in its own Run.
	let delegated = snapshot.delegated_users()?;
	for saved in &snapshot.definitions {
		if saved.identity.registry_deployment != scope.node_id() {
			continue;
		}
		let is_reference_embedding = agent
			.reference_embedding
			.as_ref()
			.is_some_and(|reference| saved.identity.local() == *reference);
		if is_reference_embedding
			&& !scope
				.reference_embedding_available(&saved.identity.local())
				.await?
		{
			return Err(Error::Invalid(
				"reference embedding is unavailable; register a new agent version".into(),
			));
		}
		let current =
			if aidash_domain::registry::bindings::sources::is_selection_decider(&saved.definition)
				|| aidash_domain::registry::bindings::defaults::is_compaction_decider(
					&saved.definition,
				) {
				// Dispatch approval may be deferred; readable immutable metadata may not.
				scope.metadata(&saved.identity.local()).await?
			} else {
				match delegated.get(&saved.identity) {
					Some(users) => {
						let users = users.iter().map(|user| user.local()).collect::<Vec<_>>();
						scope
							.closure_catalog(&saved.identity.local(), "registry.read", &users)
							.await?
					}
					None => {
						scope
							.catalog(&saved.identity.local(), "registry.read")
							.await?
					}
				}
			};
		if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?) != saved.digest {
			return Err(if is_reference_embedding {
				Error::Invalid(
					"reference embedding is unavailable; register a new agent version".into(),
				)
			} else {
				Error::Conflict("admitted definition changed".into())
			});
		}
	}

	Ok(agent)
}
#[cfg(test)]
mod tests;
