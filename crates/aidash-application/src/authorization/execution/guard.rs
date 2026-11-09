//! Recheck current execution and every pinned dependency before accepting worker output.
use crate::{
	Error, Result,
	ports::authorization::guard::{RunGuardScope, UseRecheckScope},
};
use aidash_domain::{
	RunMetadata,
	registry::{AgentConfig, EntityRef, Entry},
};

/// What a [`recheck`] of the definitions a Run uses found.
pub enum UseRecheck {
	/// The Run's Agent and everything in its Binding may still be used.
	Usable {
		entry: Box<Entry>,
		agent: Box<AgentConfig>,
	},
	/// The first definition the Run may not use.
	Blocked(EntityRef),
	/// The Agent's reference embedding is missing or no longer approved. The Run may not use it,
	/// as with [`Self::Blocked`]; the difference is what [`authorize`] answers, which is to
	/// register a new Agent version rather than to refuse.
	EmbeddingUnavailable(EntityRef),
}

impl UseRecheck {
	/// The definition the Run may not use, if there is one.
	pub fn blocked(self) -> Option<EntityRef> {
		match self {
			Self::Usable { .. } => None,
			Self::Blocked(reference) | Self::EmbeddingUnavailable(reference) => Some(reference),
		}
	}
}

fn embedding_unavailable() -> Error {
	Error::Invalid("reference embedding is unavailable; register a new agent version".into())
}

/// The first definition the Run may not use, or none: its own Agent
/// (`agent.execute`) and every definition of its Binding snapshot
/// (`registry.read`) under the current Catalog Approval and policy (#769).
///
/// This is the Use Recheck. A definition the Run only read from another Run is
/// not part of it.
pub async fn use_recheck(
	scope: &mut dyn UseRecheckScope,
	run: &RunMetadata,
) -> Result<Option<EntityRef>> {
	Ok(recheck(scope, run, true).await?.blocked())
}

/// [`use_recheck`] that also hands back what [`authorize`] needs of the Agent.
/// Without `read_context` only the Agent itself is checked, as failure delivery
/// cannot rebuild an inference context.
pub async fn recheck(
	scope: &mut dyn UseRecheckScope,
	run: &RunMetadata,
	read_context: bool,
) -> Result<UseRecheck> {
	let reference = EntityRef {
		id: run.agent_id.clone(),
		version: run.agent_version.clone(),
	};
	let entry = match scope.catalog(&reference, "agent.execute").await {
		Ok(entry) => entry,
		Err(Error::Forbidden) => return Ok(UseRecheck::Blocked(reference)),
		Err(error) => return Err(error),
	};
	if !read_context {
		let agent = AgentConfig::from_definition(serde_json::from_value(entry.config.clone())?);
		return Ok(UseRecheck::Usable {
			entry: Box::new(entry),
			agent: Box::new(agent),
		});
	}
	let snapshot = scope.binding_snapshot(run).await?;
	let agent = AgentConfig::from_snapshot(&snapshot)?;
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
			return Ok(UseRecheck::EmbeddingUnavailable(saved.identity.local()));
		}
		let current = if aidash_domain::registry::bindings::sources::is_selection_decider(
			&saved.definition,
		) {
			// Dispatch approval may be deferred; readable immutable metadata may not.
			scope.metadata(&saved.identity.local()).await
		} else {
			match delegated.get(&saved.identity) {
				Some(users) => {
					let users = users.iter().map(|user| user.local()).collect::<Vec<_>>();
					scope
						.closure_catalog(&saved.identity.local(), "registry.read", &users)
						.await
				}
				None => {
					scope
						.catalog(&saved.identity.local(), "registry.read")
						.await
				}
			}
		};
		let current = match current {
			Ok(current) => current,
			Err(Error::Forbidden) => return Ok(UseRecheck::Blocked(saved.identity.local())),
			Err(error) => return Err(error),
		};
		if aidash_domain::registry::rules::digest(&serde_json::to_value(current)?) != saved.digest {
			return Err(if is_reference_embedding {
				embedding_unavailable()
			} else {
				Error::Conflict("admitted definition changed".into())
			});
		}
	}
	Ok(UseRecheck::Usable {
		entry: Box::new(entry),
		agent: Box::new(agent),
	})
}

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
	let (entry, agent) = match recheck(scope, run, read_context).await? {
		UseRecheck::Usable { entry, agent } => (entry, agent),
		UseRecheck::Blocked(_) => return Err(Error::Forbidden),
		UseRecheck::EmbeddingUnavailable(_) => return Err(embedding_unavailable()),
	};
	super::require_agent(scope.bundle(), &run.executor_subject(scope.node_id()))?;
	scope.check_pinned(&entry).await?;
	if !read_context {
		// Failure delivery only closes the Task under current Task, Agent and
		// installation authority. It cannot infer, read Sources or invoke Tools.
		// A damaged inference context must not strand that durable obligation.
		return Ok(*agent);
	}
	if agent.needs_context_authority() {
		scope.context_authority(run).await?;
	}
	Ok(*agent)
}
#[cfg(test)]
mod tests;
