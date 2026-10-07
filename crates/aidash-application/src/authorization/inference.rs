//! Current provider authority is identical for direct worker inference and resumed execution.
use crate::{Error, Result, ports::authorization::inference::InferenceScope};
use aidash_domain::{
	RunMetadata,
	registry::{AgentConfig, EntityRef},
};
pub async fn authorize(
	scope: &mut dyn InferenceScope,
	run: &RunMetadata,
	agent: &AgentConfig,
) -> Result<()> {
	if scope.remote() {
		scope.catalog(&agent.model, "model.infer").await?;
		for skill in &agent.skills {
			scope.catalog(skill, "skill.use").await?;
		}
		return Ok(());
	}
	if agent.core_capabilities.enabled() {
		scope.context_authority(run).await?;
	}
	let node = scope.node_id().ok_or(Error::Forbidden)?.to_owned();
	scope
		.require_live(
			&node,
			run,
			&EntityRef {
				id: run.agent_id.clone(),
				version: run.agent_version.clone(),
			},
		)
		.await?;
	scope.catalog(&agent.model, "model.infer").await?;
	for skill in &agent.skills {
		scope.catalog(skill, "skill.use").await?;
	}
	if agent.memory.is_none() || agent.allow_cross_conversation_memory == Some(false) {
		return Ok(());
	}
	let resource = scope.memory_resource(run).await?;
	scope.require(&resource, "memory.read").await
}
#[cfg(test)]
mod tests;
