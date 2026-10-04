//! Current tool authority, plugin delegation, and builtin visibility are use cases.
use crate::{Error, Result, ports::authorization::tools::AgentToolRepository};
use aidash_domain::{
	RunMetadata,
	provider::ToolCall,
	registry::{AgentConfig, EntityRef},
	tool::ToolConfig,
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
	if matches!(kind, "memory" | "run" | "generation_policy") {
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
) -> Result<()> {
	refresh(repository).await?;
	if !configuration.permits_builtin(&call.name) {
		return Err(Error::Forbidden);
	}
	let mut scope = repository.lease().await?;
	if let Some(index) = call
		.name
		.strip_prefix("plugin_")
		.and_then(|i| i.parse::<usize>().ok())
	{
		let reference = configuration.tools.get(index).ok_or(Error::Forbidden)?;
		let entry = scope.catalog(reference, "tool.invoke").await?;
		if let ToolConfig::Agent { node_id, agent } = serde_json::from_value(entry.config)? {
			if configuration.allow_task_delegation == Some(false) {
				return Err(Error::Forbidden);
			}
			if node_id != run.home_node {
				return Err(Error::Forbidden);
			}
			scope.catalog(&agent, "agent.execute").await?;
			let resource = scope.resource("workspace", &run.workspace_id.to_string(), json!({}));
			scope.require(&resource, "task.create").await?;
		}
		return Ok(());
	}
	let resource = scope.resource("tool", &format!("builtin:{}", call.name), json!({}));
	scope.require(&resource, "tool.invoke").await?;
	if matches!(
		call.name.as_str(),
		"file_read"
			| "file_search"
			| "shell" | "shell_poll"
			| "shell_cancel"
			| "apply_patch"
			| "file_share"
			| "outbound_get"
			| "code_interpreter"
			| "python_install"
			| "python_poll"
			| "python_cancel"
	) {
		if !configuration.core_capabilities.permits(&call.name) {
			return Err(Error::Forbidden);
		}
		return Ok(());
	}
	if matches!(call.name.as_str(), "skill_list" | "skill_load")
		|| (call.name == "skill_read" && call.arguments.get("skill_id").is_some())
	{
		return if configuration.core_capabilities.skills {
			Ok(())
		} else {
			Err(Error::Forbidden)
		};
	}
	let (action, kind, id) = match call.name.as_str() {
		"task_create" => ("task.create", "workspace", run.workspace_id.to_string()),
		"task_assign" => {
			let id = call.arguments["policy_id"]
				.as_str()
				.ok_or_else(|| Error::Invalid("missing generation policy".into()))?;
			("generation.request", "generation_policy", id.to_owned())
		}
		"task_delegate" => {
			let id = call.arguments["task_id"]
				.as_str()
				.and_then(|s| s.parse::<Uuid>().ok())
				.ok_or_else(|| Error::Invalid("invalid task id".into()))?;
			if call.arguments["node_id"] != run.home_node {
				return Err(Error::Forbidden);
			}
			let reference: EntityRef = serde_json::from_value(call.arguments["agent"].clone())?;
			scope.catalog(&reference, "agent.execute").await?;
			("task.delegate", "task", id.to_string())
		}
		"artifact_publish" => ("artifact.create", "artifact", run.task_id.to_string()),
		"workspace_message" => ("message.create", "workspace", run.workspace_id.to_string()),
		"memory_write" => ("memory.write", "memory", run.agent_id.clone()),
		"human_request" => ("human.request", "run", run.id.to_string()),
		"skill_read" => {
			let reference: EntityRef = serde_json::from_value(call.arguments["skill"].clone())
				.map_err(|error| Error::Invalid(error.to_string()))?;
			if !configuration.skills.contains(&reference) {
				return Err(Error::Forbidden);
			}
			let entry = scope.catalog(&reference, "skill.use").await?;
			if entry.kind != "skill" {
				return Err(Error::Forbidden);
			}
			return Ok(());
		}
		"agent_discover" | "workspace_observe" | "workspace_read" | "workspace_wait" => {
			return Ok(());
		}
		_ => return Err(Error::Forbidden),
	};
	let resource =
		protected_resource(scope.as_mut(), repository.is_remote(), run, kind, &id).await?;
	scope.require(&resource, action).await
}

/// Apply removals immediately so a later adapter failure retains earlier filtering.
pub async fn filter<T: Send>(
	repository: &dyn AgentToolRepository,
	agent: &AgentConfig,
	tools: &mut BTreeMap<String, T>,
) -> Result<()> {
	if repository.is_remote() {
		tools.retain(|name, _| {
			matches!(
				name.as_str(),
				"task_create"
					| "artifact_publish"
					| "workspace_message"
					| "workspace_observe"
					| "workspace_read"
					| "workspace_wait"
					| "skill_read"
			) || name.starts_with("plugin_")
		});
	}
	let mut scope = repository.lease().await?;
	for name in tools.keys().cloned().collect::<Vec<_>>() {
		if !agent.core_capabilities.permits(&name) {
			continue;
		}
		let resource = scope.resource("tool", &format!("builtin:{name}"), json!({}));
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
