//! Current tool authority, plugin delegation, and builtin visibility are use cases.
use crate::{Error, Result, ports::authorization::tools::AgentToolRepository};
use aidash_domain::{
	RunMetadata,
	provider::ToolCall,
	registry::{AgentConfig, EntityRef},
	tool::{AuthorizationRequirement, ResourceTarget, ToolConfig, ToolContract, ToolIdentity},
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Replace foreign authority under one retained lease before evaluating a tool flag.
pub async fn refresh(repository: &dyn AgentToolRepository) -> Result<()> {
	if !repository.is_remote() {
		return Ok(());
	}
	let mut scope = repository.lease().await?;
	if scope.transaction_active() {
		scope.suspend().await?;
	}
	if !scope.replace_remote_authority().await? {
		return Err(Error::Forbidden);
	}
	Ok(())
}

/// Foreign execution can address only its admitted Task and Workspace.
pub fn remote_identifier(run: &RunMetadata, kind: &str, id: &str) -> Result<String> {
	if matches!(kind, "memory" | "generation_policy") || kind == "run" && id != run.id.to_string() {
		return Err(Error::Forbidden);
	}
	if (kind == "task" && id != run.task_id.to_string())
		|| (kind == "workspace" && id != run.workspace_id.to_string())
	{
		return Err(Error::Forbidden);
	}
	Ok(format!("{}/{kind}s/{id}", run.home_node))
}

pub async fn authorize(
	repository: &dyn AgentToolRepository,
	run: &RunMetadata,
	configuration: &AgentConfig,
	call: &ToolCall,
	contract: &ToolContract,
) -> Result<()> {
	refresh(repository).await?;
	if contract
		.authorization
		.flag
		.is_some_and(|flag| !flag.permitted(configuration))
	{
		return Err(Error::Forbidden);
	}
	let mut scope = repository.lease().await?;
	let current;
	let authorization = match &contract.identity {
		ToolIdentity::Descriptor(reference) => {
			let entry = scope.catalog(&reference.local(), "tool.invoke").await?;
			let descriptor: aidash_domain::tool::providers::ToolDescriptor =
				serde_json::from_value(entry.config)?;
			current = descriptor.declared_contract(reference.clone())?;
			if current != *contract {
				return Err(Error::Conflict("admitted provider contract changed".into()));
			}
			&current.authorization
		}
		ToolIdentity::Registry(reference) => {
			if !configuration.tools.contains(reference) {
				return Err(Error::Forbidden);
			}
			let entry = scope.catalog(reference, "tool.invoke").await?;
			let config: ToolConfig = serde_json::from_value(entry.config)?;
			current = ToolContract::registry(reference.clone(), &config);
			&current.authorization
		}
		ToolIdentity::Builtin(grant) => {
			let resource = scope.resource("tool", grant, json!({}));
			scope.require(&resource, "tool.invoke").await?;
			&contract.authorization
		}
	};
	let legacy_skill = authorization
		.requirements
		.contains(&AuthorizationRequirement::ConfiguredSkill)
		&& call.arguments.get("skill_id").is_none();
	if !legacy_skill
		&& authorization
			.core
			.is_some_and(|permission| !permission.permitted(&configuration.core_capabilities))
	{
		return Err(Error::Forbidden);
	}
	for requirement in &authorization.requirements {
		match requirement {
			AuthorizationRequirement::AgentExecution { node_id, agent } => {
				if configuration.allow_task_delegation == Some(false) || node_id != &run.home_node {
					return Err(Error::Forbidden);
				}
				scope.catalog(agent, "agent.execute").await?;
				let resource =
					scope.resource("workspace", &run.workspace_id.to_string(), json!({}));
				scope.require(&resource, "task.create").await?;
			}
			AuthorizationRequirement::ConfiguredSkill => {
				if !legacy_skill {
					continue;
				}
				let reference: EntityRef = serde_json::from_value(call.arguments["skill"].clone())
					.map_err(|e| Error::Invalid(e.to_string()))?;
				if !configuration.skills.contains(&reference) {
					return Err(Error::Forbidden);
				}
				if scope.catalog(&reference, "skill.use").await?.kind != "skill" {
					return Err(Error::Forbidden);
				}
			}
			AuthorizationRequirement::LocalDelegation => {
				call.arguments["task_id"]
					.as_str()
					.and_then(|s| s.parse::<Uuid>().ok())
					.ok_or_else(|| Error::Invalid("invalid task id".into()))?;
				if call.arguments["node_id"] != run.home_node {
					return Err(Error::Forbidden);
				}
				let reference: EntityRef = serde_json::from_value(call.arguments["agent"].clone())?;
				scope.catalog(&reference, "agent.execute").await?;
			}
			AuthorizationRequirement::Resource {
				action,
				kind,
				target,
			} => {
				let id = match target {
					ResourceTarget::Workspace => run.workspace_id.to_string(),
					ResourceTarget::Task => run.task_id.to_string(),
					ResourceTarget::Agent => run.agent_id.clone(),
					ResourceTarget::Run => run.id.to_string(),
					ResourceTarget::ArgumentUuid(field) => call.arguments[field]
						.as_str()
						.and_then(|s| s.parse::<Uuid>().ok())
						.ok_or_else(|| Error::Invalid("invalid task id".into()))?
						.to_string(),
					ResourceTarget::Argument(field) => call.arguments[field]
						.as_str()
						.ok_or_else(|| Error::Invalid("missing generation policy".into()))?
						.to_owned(),
				};
				let resource =
					protected_resource(scope.as_mut(), repository.is_remote(), run, kind, &id)
						.await?;
				scope.require(&resource, action).await?;
			}
		}
	}
	Ok(())
}

/// Apply removals immediately so a later adapter failure retains earlier filtering.
pub async fn filter<T: Send>(
	repository: &dyn AgentToolRepository,
	agent: &AgentConfig,
	tools: &mut BTreeMap<String, T>,
	contract: impl Fn(&T) -> ToolContract,
) -> Result<()> {
	if repository.is_remote() {
		tools.retain(|_, tool| contract(tool).remote_exposure);
	}
	let mut scope = repository.lease().await?;
	for name in tools.keys().cloned().collect::<Vec<_>>() {
		let declaration = contract(&tools[&name]);
		if !declaration
			.authorization
			.core
			.is_some_and(|permission| permission.permitted(&agent.core_capabilities))
		{
			continue;
		}
		let ToolIdentity::Builtin(grant) = declaration.identity else {
			continue;
		};
		let resource = scope.resource("tool", &grant, json!({}));
		match scope.require(&resource, "tool.invoke").await {
			Ok(()) => {}
			Err(Error::Forbidden | Error::NotFound(_)) => {
				tools.remove(&name);
			}
			Err(error) => return Err(error),
		}
	}
	Ok(())
}

async fn protected_resource(
	scope: &mut dyn crate::ports::authorization::tools::AgentToolScope,
	remote: bool,
	run: &RunMetadata,
	kind: &str,
	id: &str,
) -> Result<aidash_domain::policy::Resource> {
	Ok(if remote {
		scope.resource(
			kind,
			&remote_identifier(run, kind, id)?,
			scope.context().clone(),
		)
	} else {
		match kind {
			"task" => {
				let task = scope
					.task_read(id.parse().map_err(|_| Error::Forbidden)?)
					.await?;
				scope.task_resource(&task).await?
			}
			"artifact" => {
				let creator = scope.subjects().last().ok_or(Error::Forbidden)?.clone();
				scope
					.artifact_creation_resource(run.task_id, &creator)
					.await?
			}
			"memory" => scope.memory_resource(run).await?,
			_ => scope.resource(kind, id, json!({})),
		}
	})
}

/// Apply the same resource authority to worker effects and explicit control-plane actions.
pub async fn action(
	repository: &dyn AgentToolRepository,
	run: &RunMetadata,
	action: &str,
	kind: &str,
	id: &str,
) -> Result<()> {
	refresh(repository).await?;
	let mut scope = repository.lease().await?;
	let resource =
		protected_resource(scope.as_mut(), repository.is_remote(), run, kind, id).await?;
	scope.require(&resource, action).await
}

/// A worker may disclose only a saved request for its own Run and Workspace.
pub async fn human_read(
	repository: &dyn AgentToolRepository,
	run: &RunMetadata,
	id: Uuid,
) -> Result<()> {
	if repository.is_remote() {
		return Err(Error::Forbidden);
	}
	let mut scope = repository.lease().await?;
	let request = scope
		.human_request(run, id)
		.await?
		.ok_or(Error::Forbidden)?;
	let resource = scope.human_resource(&request).await?;
	scope.require(&resource, "human.read").await
}

#[cfg(test)]
mod tests;
